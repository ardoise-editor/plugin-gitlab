//! What is asked of GitLab: reading what is on show, and what the controls of
//! the panel do.

use std::future::Future;

use ardoise_plugin::protocol::{DiffFile, DiffMode, DiffTabParams, UiEvent};

use crate::api::{Client, Error};
use crate::state::{Shown, Tab, FILTERS};
use crate::{lock, Ctx};

/// Reads what the tab on show needs.
pub async fn read(ctx: &Ctx, client: &Client, generation: u64) -> Result<(), Error> {
    let (tab, filter, shown, pipeline, known) = ctx.with(|s| {
        (
            s.tab,
            s.filter,
            s.shown.as_ref().map(|m| m.request.iid),
            s.pipeline.as_ref().map(|(id, _)| *id),
            !s.default_branch.is_empty(),
        )
    });
    match tab {
        Tab::MergeRequests => {
            if !known {
                let project = client.project().await?;
                let branches = client.branches().await.unwrap_or_default();
                ctx.answer(generation, |s| {
                    s.default_branch = project.default_branch.unwrap_or_default();
                    s.branches = branches;
                    // A form opened before the branches were known.
                    if s.new.as_ref().is_some_and(|n| n.target.is_empty()) {
                        s.new = Some(s.draft());
                    }
                });
            }
            match shown {
                Some(iid) => read_request(ctx, client, generation, iid).await?,
                None => {
                    let found = client.merge_requests(FILTERS[filter]).await?;
                    ctx.answer(generation, |s| {
                        // Another filter may have been picked meanwhile.
                        if s.filter == filter {
                            s.requests = Some(found);
                        }
                    });
                }
            }
        }
        Tab::Pipelines => match pipeline {
            Some(id) => {
                let jobs = client.jobs(id).await?;
                ctx.answer(generation, |s| {
                    if let Some((on_show, slot)) = &mut s.pipeline {
                        if *on_show == id {
                            *slot = Some(jobs);
                        }
                    }
                });
                // The list tells whether it still runs.
                let found = client.pipelines().await?;
                ctx.answer(generation, |s| s.pipelines = Some(found));
            }
            None => {
                let found = client.pipelines().await?;
                ctx.answer(generation, |s| s.pipelines = Some(found));
            }
        },
        Tab::Tags => {
            let found = client.tags().await?;
            ctx.answer(generation, |s| s.tags = Some(found));
        }
    }
    Ok(())
}

async fn read_request(ctx: &Ctx, client: &Client, generation: u64, iid: u64) -> Result<(), Error> {
    let request = client.merge_request(iid).await?;
    let files = client.diffs(iid).await?;
    let discussions = client.discussions(iid).await?;
    // A project without pipelines, or no right to see them: none.
    let pipelines = client.mr_pipelines(iid).await.unwrap_or_default();
    ctx.answer(generation, |s| {
        if let Some(shown) = s.shown.as_mut().filter(|m| m.request.iid == iid) {
            shown.request = request;
            shown.files = Some(files);
            shown.discussions = Some(discussions);
            shown.pipelines = pipelines;
        }
    });
    Ok(())
}

/// Does something on the server: the panel waits, says how it went, and
/// reads again what it shows.
fn act<F, Fut>(ctx: &Ctx, done: String, op: F)
where
    F: FnOnce(Client) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), Error>> + Send,
{
    // One thing at a time.
    let free = ctx.with(|s| {
        let free = !s.busy;
        if free {
            s.busy = true;
            s.armed = None;
            s.notice = None;
        }
        free
    });
    if !free {
        return;
    }
    ctx.render();
    let ctx = ctx.clone();
    let generation = ctx.now();
    tokio::spawn(async move {
        let result = match ctx.client().await {
            Some(Ok(client)) => op(client).await,
            Some(Err(error)) => Err(error),
            None => Err(Error::Unauthorized),
        };
        ctx.answer(generation, |s| {
            s.busy = false;
            s.notice = Some(match &result {
                Ok(()) => (done, false),
                Err(error) => (error.to_string(), true),
            });
        });
        ctx.load().await;
    });
}

/// Reads again in the background, after the panel is drawn as it is now.
fn reload(ctx: &Ctx) {
    ctx.render();
    let ctx = ctx.clone();
    tokio::spawn(async move { ctx.load().await });
}

/// Opens a merge request in the panel.
fn open_request(ctx: &Ctx, iid: u64) {
    ctx.with(|s| {
        let found = s
            .requests
            .as_ref()
            .and_then(|all| all.iter().find(|m| m.iid == iid))
            .cloned();
        if let Some(request) = found {
            // The source branch is deleted only when the box is ticked: it
            // may be one that lives on (`develop` into `main`).
            s.shown = Some(Shown {
                request,
                ..Shown::default()
            });
            s.notice = None;
            s.armed = None;
        }
    });
    reload(ctx);
}

/// The files of the merge request on show, in a tab of diffs.
fn show_changes(ctx: &Ctx, start: usize) {
    let params = ctx.with(|s| {
        let shown = s.shown.as_ref()?;
        let files = shown.files.as_ref().filter(|f| !f.is_empty())?;
        Some(DiffTabParams {
            tab: format!("mr-{}", shown.request.iid),
            title: format!("!{} {}", shown.request.iid, shown.request.title),
            files: files
                .iter()
                .map(|f| DiffFile {
                    path: f.new_path.clone(),
                    from: f.renamed_file.then(|| f.old_path.clone()),
                    patch: f.patch(),
                    mode: DiffMode::Plain,
                    file: None,
                })
                .collect(),
            start: start.min(files.len() - 1),
        })
    });
    let Some(params) = params else {
        return;
    };
    let host = ctx.host.clone();
    // The editor answers: not on the thread that reads what it says.
    tokio::task::spawn_blocking(move || {
        if let Err(e) = host.open_diff_tab(params) {
            host.error(e.message);
        }
    });
}

/// The log of a job without the codes that color a terminal.
pub fn plain(log: &str) -> String {
    let mut out = String::with_capacity(log.len());
    let mut chars = log.chars();
    while let Some(c) = chars.next() {
        match c {
            // `ESC [ … letter`.
            '\u{1b}' => {
                if chars.next() == Some('[') {
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
            }
            '\r' => out.push('\n'),
            c => out.push(c),
        }
    }
    out.replace("\n\n", "\n")
}

fn show_log(ctx: &Ctx, job: u64) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let Some(Ok(client)) = ctx.client().await else {
            return;
        };
        match client.job_log(job).await {
            Ok(log) => {
                let host = ctx.host.clone();
                tokio::task::spawn_blocking(move || {
                    if let Err(e) = host.open_text(format!("job-{job}.log"), plain(&log), "log") {
                        host.error(e.message);
                    }
                });
            }
            Err(error) => {
                ctx.with(|s| s.notice = Some((error.to_string(), true)));
                ctx.render();
            }
        }
    });
}

/// Keeps the token typed for the server of the project, and tries it.
fn connect(ctx: &Ctx) {
    let typed = ctx.with(|s| {
        let host = s.host()?.to_string();
        let token = s.token.trim().to_string();
        let address = s.address.trim().trim_end_matches('/').to_string();
        (!token.is_empty()).then_some((host, token, address))
    });
    let Some((host, token, address)) = typed else {
        return;
    };
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let store = ctx.store.clone();
        let kept = tokio::task::spawn_blocking(move || {
            let mut store = lock(&store);
            store.set_token(&host, &token);
            if let Some(server) = store.servers.get_mut(&host) {
                server.address = address;
            }
            store.save()
        })
        .await;
        ctx.with(|s| {
            s.token.clear();
            s.link = Default::default();
            if let Ok(Err(error)) = &kept {
                s.notice = Some((error.clone(), true));
            }
        });
        ctx.render_settings();
        ctx.render();
        ctx.connect().await;
    });
}

fn forget(ctx: &Ctx, host: String) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let store = ctx.store.clone();
        let forgotten = host.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let mut store = lock(&store);
            store.forget(&forgotten);
            store.save()
        })
        .await;
        ctx.with(|s| {
            s.armed = None;
            if s.host() == Some(host.as_str()) {
                s.link = crate::state::Link::NoToken;
                s.requests = None;
                s.shown = None;
                s.pipelines = None;
                s.tags = None;
            }
        });
        ctx.render_settings();
        ctx.render();
    });
}

pub fn event(ctx: &Ctx, event: &UiEvent) {
    let action = event.action.as_str();
    let text = event.text().unwrap_or_default().to_string();
    let checked = event.checked().unwrap_or_default();

    // A button that destroys: the first click only arms it.
    if action.starts_with("confirm:") && !ctx.with(|s| s.confirm(action)) {
        ctx.render();
        ctx.render_settings();
        return;
    }
    if let Some(rest) = action.strip_prefix("confirm:") {
        return confirmed(ctx, rest);
    }

    match action {
        // Fields: kept as they are typed.
        "token" => ctx.with(|s| s.token = text),
        "address" => ctx.with(|s| s.address = text),
        "connect" => connect(ctx),
        "refresh" => {
            ctx.with(|s| {
                s.notice = None;
                // The branches are read again too.
                s.default_branch.clear();
            });
            reload(ctx);
        }
        "tab:mrs" | "tab:pipelines" | "tab:tags" => {
            ctx.with(|s| {
                s.tab = match action {
                    "tab:pipelines" => Tab::Pipelines,
                    "tab:tags" => Tab::Tags,
                    _ => Tab::MergeRequests,
                };
                s.notice = None;
                s.armed = None;
            });
            reload(ctx);
        }

        // Merge requests
        "filter" => {
            ctx.with(|s| {
                if let Some(at) = FILTERS
                    .iter()
                    .position(|f| crate::ui::filter_label(f) == text)
                {
                    s.filter = at;
                    s.requests = None;
                }
            });
            reload(ctx);
        }
        "mrs" => {
            if let Ok(iid) = text.parse() {
                open_request(ctx, iid);
            }
        }
        "mr:back" => {
            ctx.with(|s| s.shown = None);
            reload(ctx);
        }
        "mr:new" => {
            ctx.with(|s| {
                s.notice = None;
                s.new = Some(s.draft());
            });
            ctx.render();
        }
        "mr:cancel" => {
            ctx.with(|s| s.new = None);
            ctx.render();
        }
        "new_source" | "new_target" => {
            ctx.with(|s| {
                if let Some(new) = &mut s.new {
                    if action == "new_target" {
                        new.target = text;
                    } else {
                        // The title follows the branch while it is the one
                        // made from it.
                        if new.title == crate::state::title_of(&new.source) {
                            new.title = crate::state::title_of(&text);
                        }
                        new.source = text;
                    }
                }
            });
            ctx.render();
        }
        "new_title" => {
            ctx.with(|s| {
                if let Some(new) = &mut s.new {
                    new.title = text;
                }
            });
            ctx.render();
        }
        "new_description" => ctx.with(|s| {
            if let Some(new) = &mut s.new {
                new.description = text;
            }
        }),
        "new_draft" | "new_remove" | "new_squash" => {
            ctx.with(|s| {
                if let Some(new) = &mut s.new {
                    match action {
                        "new_draft" => new.draft = checked,
                        "new_remove" => new.remove_source = checked,
                        _ => new.squash = checked,
                    }
                }
            });
            ctx.render();
        }
        "mr:create" => {
            let Some(new) = ctx.with(|s| s.new.clone()) else {
                return;
            };
            let ctx2 = ctx.clone();
            act(ctx, t!("gl.mr_created").to_string(), |client| async move {
                client.create(&new).await?;
                // The form stays when GitLab refuses, with what was typed.
                ctx2.with(|s| {
                    s.new = None;
                    s.requests = None;
                });
                Ok(())
            });
        }
        "mr:link" => {
            if let Some(url) = ctx.with(|s| s.shown.as_ref().map(|m| m.request.web_url.clone())) {
                ctx.host.copy(url);
                ctx.with(|s| s.notice = Some((t!("gl.link_copied").to_string(), false)));
                ctx.render();
            }
        }
        "mr:changes" => show_changes(ctx, 0),
        "mr_files" => {
            if let Ok(at) = text.parse() {
                show_changes(ctx, at);
            }
        }
        "mr:approve" | "mr:unapprove" => {
            let Some((iid, sha)) = seen(ctx) else {
                return;
            };
            let approve = action == "mr:approve";
            let done = if approve {
                t!("gl.approved")
            } else {
                t!("gl.unapproved")
            };
            act(ctx, done.to_string(), move |client| async move {
                if approve {
                    client.approve(iid, &sha).await
                } else {
                    client.unapprove(iid).await
                }
            });
        }
        "mr:reopen" => {
            let Some((iid, _)) = seen(ctx) else {
                return;
            };
            act(ctx, t!("gl.done").to_string(), move |client| async move {
                client.set_open(iid, true).await
            });
        }
        "merge_squash" | "merge_remove" | "merge_later" => {
            ctx.with(|s| {
                if let Some(shown) = &mut s.shown {
                    match action {
                        "merge_squash" => shown.merge.squash = checked,
                        "merge_remove" => shown.merge.remove_source = checked,
                        _ => shown.merge.when_pipeline_succeeds = checked,
                    }
                    s.armed = None;
                }
            });
            ctx.render();
        }

        // Comments
        "comment" => ctx.with(|s| {
            if let Some(shown) = &mut s.shown {
                shown.comment = text;
            }
        }),
        "comment:send" => {
            let said = ctx.with(|s| {
                let shown = s.shown.as_ref()?;
                let body = shown.comment.trim().to_string();
                (!body.is_empty()).then_some((shown.request.iid, body))
            });
            let Some((iid, body)) = said else {
                return;
            };
            let ctx2 = ctx.clone();
            act(
                ctx,
                t!("gl.comment_sent").to_string(),
                move |client| async move {
                    client.comment(iid, &body).await?;
                    ctx2.with(|s| {
                        if let Some(shown) = &mut s.shown {
                            shown.comment.clear();
                        }
                    });
                    Ok(())
                },
            );
        }
        "reply" => ctx.with(|s| {
            if let Some((_, answer)) = s.shown.as_mut().and_then(|m| m.reply.as_mut()) {
                *answer = text;
            }
        }),
        "reply:send" => {
            let said = ctx.with(|s| {
                let shown = s.shown.as_ref()?;
                let (thread, answer) = shown.reply.as_ref()?;
                let body = answer.trim().to_string();
                (!body.is_empty()).then_some((shown.request.iid, thread.clone(), body))
            });
            let Some((iid, thread, body)) = said else {
                return;
            };
            let ctx2 = ctx.clone();
            act(
                ctx,
                t!("gl.comment_sent").to_string(),
                move |client| async move {
                    client.reply(iid, &thread, &body).await?;
                    ctx2.with(|s| {
                        if let Some(shown) = &mut s.shown {
                            shown.reply = None;
                        }
                    });
                    Ok(())
                },
            );
        }

        // Pipelines and jobs
        "run_ref" => {
            ctx.with(|s| s.run_ref = text);
            ctx.render();
        }
        "pipeline:run" => {
            let reference = ctx.with(|s| s.run_ref.trim().to_string());
            if reference.is_empty() {
                return;
            }
            act(
                ctx,
                t!("gl.pipeline_started").to_string(),
                move |client| async move { client.run_pipeline(&reference).await },
            );
        }
        "pipelines" => {
            if let Ok(id) = text.parse() {
                ctx.with(|s| s.pipeline = Some((id, None)));
                reload(ctx);
            }
        }
        "pipeline:back" => {
            ctx.with(|s| s.pipeline = None);
            reload(ctx);
        }
        "tag_name" => ctx.with(|s| s.tag_name = text),
        "tag_ref" => ctx.with(|s| s.tag_ref = text),
        "tag_message" => ctx.with(|s| s.tag_message = text),
        "tag:create" => {
            let (name, reference, message) = ctx.with(|s| {
                let reference = if s.tag_ref.trim().is_empty() {
                    s.default_branch.clone()
                } else {
                    s.tag_ref.clone()
                };
                (s.tag_name.clone(), reference, s.tag_message.clone())
            });
            if name.trim().is_empty() || reference.trim().is_empty() {
                ctx.with(|s| s.notice = Some((t!("gl.tag_needs").to_string(), true)));
                ctx.render();
                return;
            }
            let ctx2 = ctx.clone();
            act(
                ctx,
                t!("gl.tag_created").to_string(),
                move |client| async move {
                    client.create_tag(&name, &reference, &message).await?;
                    ctx2.with(|s| {
                        s.tag_name.clear();
                        s.tag_message.clear();
                    });
                    Ok(())
                },
            );
        }
        other => with_argument(ctx, other),
    }
}

/// The merge request on show and the commit that was seen at its head.
fn seen(ctx: &Ctx) -> Option<(u64, String)> {
    ctx.with(|s| {
        let m = &s.shown.as_ref()?.request;
        Some((m.iid, m.sha.clone().unwrap_or_default()))
    })
}

/// The actions that carry what they are about: `verb:argument`.
fn with_argument(ctx: &Ctx, action: &str) {
    if let Some(thread) = action.strip_prefix("reply:open:") {
        ctx.with(|s| {
            if let Some(shown) = &mut s.shown {
                shown.reply = Some((thread.to_string(), String::new()));
            }
        });
        ctx.render();
        return;
    }
    for (verb, resolved) in [("resolve:", true), ("unresolve:", false)] {
        if let Some(thread) = action.strip_prefix(verb) {
            let Some((iid, _)) = seen(ctx) else {
                return;
            };
            let thread = thread.to_string();
            act(ctx, t!("gl.done").to_string(), move |client| async move {
                client.resolve(iid, &thread, resolved).await
            });
            return;
        }
    }
    let mut parts = action.split(':');
    let (Some(what), Some(verb), Some(Ok(id))) = (
        parts.next(),
        parts.next(),
        parts.next().map(str::parse::<u64>),
    ) else {
        return;
    };
    match (what, verb) {
        ("pipeline", "show") => {
            ctx.with(|s| {
                s.tab = Tab::Pipelines;
                s.pipeline = Some((id, None));
            });
            reload(ctx);
        }
        ("pipeline", "retry" | "cancel") => {
            let verb = verb.to_string();
            act(ctx, t!("gl.done").to_string(), move |client| async move {
                client.pipeline_action(id, &verb).await
            });
        }
        ("job", "log") => show_log(ctx, id),
        ("job", "retry" | "cancel") => {
            let verb = verb.to_string();
            act(ctx, t!("gl.done").to_string(), move |client| async move {
                client.job_action(id, &verb).await
            });
        }
        _ => {}
    }
}

/// What a button clicked twice does.
fn confirmed(ctx: &Ctx, action: &str) {
    if let Some(host) = action.strip_prefix("forget:") {
        return forget(ctx, host.to_string());
    }
    if let Some(name) = action.strip_prefix("tag:delete:") {
        let name = name.to_string();
        return act(
            ctx,
            t!("gl.tag_deleted").to_string(),
            move |client| async move { client.delete_tag(&name).await },
        );
    }
    if let Some(id) = action.strip_prefix("pipeline:delete:") {
        let Ok(id) = id.parse() else {
            return;
        };
        let ctx2 = ctx.clone();
        return act(ctx, t!("gl.done").to_string(), move |client| async move {
            client.delete_pipeline(id).await?;
            ctx2.with(|s| s.pipeline = None);
            Ok(())
        });
    }
    if let Some(id) = action.strip_prefix("job:play:") {
        let Ok(id) = id.parse() else {
            return;
        };
        return act(ctx, t!("gl.done").to_string(), move |client| async move {
            client.job_action(id, "play").await
        });
    }
    let Some((iid, sha)) = seen(ctx) else {
        return;
    };
    match action {
        "mr:merge" => {
            let how = ctx.with(|s| {
                let shown = s.shown.as_ref();
                let mut how = shown.map(|m| m.merge.clone()).unwrap_or_default();
                // Offered only while a pipeline runs.
                how.when_pipeline_succeeds &= shown.is_some_and(|m| {
                    m.request
                        .head_pipeline
                        .as_ref()
                        .is_some_and(|p| p.running())
                });
                how
            });
            act(ctx, t!("gl.merged").to_string(), move |client| async move {
                client.merge(iid, &sha, &how).await
            });
        }
        "mr:close" => act(ctx, t!("gl.done").to_string(), move |client| async move {
            client.set_open(iid, false).await
        }),
        "mr:delete" => {
            let ctx2 = ctx.clone();
            act(
                ctx,
                t!("gl.mr_deleted").to_string(),
                move |client| async move {
                    client.delete(iid).await?;
                    ctx2.with(|s| {
                        s.shown = None;
                        s.requests = None;
                    });
                    Ok(())
                },
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_log_loses_the_colors_of_the_terminal() {
        assert_eq!(
            plain("\u{1b}[32;1m$ cargo test\u{1b}[0;m\r\n\u{1b}[0Kok\n"),
            "$ cargo test\nok\n"
        );
    }
}
