//! The panel and the settings, drawn from the state.

use ardoise_plugin::ui::{
    button, checkbox, column, editor, input, item, list, row, scroll, select, separator, text,
    ButtonStyle, Node, TextStyle,
};

use crate::api::{Discussion, Job, MergeRequest, NewMergeRequest, Pipeline};
use crate::state::{Link, Shown, State, Tab, FILTERS};

fn muted(said: impl Into<String>) -> Node {
    text(said.into()).style(TextStyle::Muted)
}

fn heading(said: impl Into<String>) -> Node {
    text(said.into()).style(TextStyle::Heading)
}

fn tool(label: impl Into<String>, action: impl Into<String>, enabled: bool) -> Node {
    button(label.into(), action.into())
        .button_style(ButtonStyle::Ghost)
        .enabled(enabled)
}

fn primary(label: impl Into<String>, action: impl Into<String>, enabled: bool) -> Node {
    button(label.into(), action.into())
        .button_style(ButtonStyle::Primary)
        .enabled(enabled)
}

/// A button that asks to be clicked twice: `action` starts with `confirm:`.
fn armed(state: &State, label: impl Into<String>, action: String) -> Node {
    if state.armed.as_deref() == Some(action.as_str()) {
        button(t!("gl.confirm").to_string(), action)
            .button_style(ButtonStyle::Danger)
            .enabled(!state.busy)
    } else {
        tool(label, action, !state.busy)
    }
}

pub fn filter_label(filter: &str) -> String {
    match filter {
        "opened" => t!("gl.state_opened"),
        "merged" => t!("gl.state_merged"),
        "closed" => t!("gl.state_closed"),
        _ => t!("gl.state_all"),
    }
    .to_string()
}

/// A mark that says how a pipeline or a job went.
pub fn mark(status: &str) -> &'static str {
    match status {
        "success" => "✓",
        "failed" => "✗",
        "running" => "▶",
        "canceled" | "canceling" => "⊘",
        "skipped" => "»",
        "manual" => "⏸",
        _ => "…",
    }
}

fn day(date: &str) -> &str {
    date.get(..10).unwrap_or(date)
}

pub fn panel(state: &State) -> Node {
    let Some(checkout) = &state.checkout else {
        return column([muted(t!("gl.no_project").to_string())]).padding(8.0);
    };
    let project = &checkout.project;
    let mut parts = Vec::new();
    let ready = matches!(state.link, Link::Ready(_));
    let mut head = Vec::new();
    for (tab, id, label) in [
        (Tab::MergeRequests, "tab:mrs", t!("gl.tab_mrs")),
        (Tab::Pipelines, "tab:pipelines", t!("gl.tab_pipelines")),
        (Tab::Tags, "tab:tags", t!("gl.tab_tags")),
    ] {
        let style = if ready && state.tab == tab {
            ButtonStyle::Primary
        } else {
            ButtonStyle::Ghost
        };
        head.push(
            button(label.to_string(), id)
                .button_style(style)
                .enabled(ready),
        );
    }
    head.push(muted(format!("{}/{}", project.host, project.path)));
    head.push(tool(t!("gl.refresh").to_string(), "refresh", !state.busy));
    parts.push(row(head).spacing(6.0));
    if let Some((said, error)) = &state.notice {
        parts.push(text(said.clone()).style(if *error {
            TextStyle::Error
        } else {
            TextStyle::Muted
        }));
    }
    if state.busy {
        parts.push(muted(t!("gl.busy").to_string()));
    }
    match &state.link {
        Link::Checking => parts.push(muted(t!("gl.checking").to_string())),
        Link::NoToken => parts.extend(connect(state, &project.host)),
        Link::Failed(error) => {
            parts.push(text(error.clone()).style(TextStyle::Error));
            parts.extend(connect(state, &project.host));
        }
        Link::Ready(_) => parts.push(match state.tab {
            Tab::MergeRequests => merge_requests(state),
            Tab::Pipelines => pipelines(state),
            Tab::Tags => tags(state),
        }),
    }
    column(parts).spacing(8.0).padding(6.0)
}

/// The token of the server is asked for.
fn connect(state: &State, host: &str) -> Vec<Node> {
    vec![
        heading(t!("gl.connect_title", host = host).to_string()),
        muted(t!("gl.connect_help", host = host).to_string()),
        input("token", state.token.clone(), t!("gl.token").to_string())
            .secret()
            .on_submit("connect"),
        input(
            "address",
            state.address.clone(),
            t!("gl.address", host = host).to_string(),
        ),
        row([primary(
            t!("gl.connect").to_string(),
            "connect",
            !state.busy,
        )]),
    ]
}

// ─── Merge requests ─────────────────────────────────────────────────────────

fn merge_requests(state: &State) -> Node {
    if let Some(new) = &state.new {
        return new_request(state, new);
    }
    if let Some(shown) = &state.shown {
        return one_request(state, shown);
    }
    let labels: Vec<String> = FILTERS.iter().map(|f| filter_label(f)).collect();
    let mut parts = vec![row([
        select("filter", labels, Some(filter_label(FILTERS[state.filter]))),
        primary(t!("gl.mr_new").to_string(), "mr:new", !state.busy),
    ])
    .spacing(8.0)];
    match &state.requests {
        None => parts.push(muted(t!("gl.loading").to_string())),
        Some(found) if found.is_empty() => parts.push(muted(t!("gl.mr_none").to_string())),
        Some(found) => parts.push(list(
            "mrs",
            found.iter().map(|m| {
                item(m.iid.to_string(), format!("!{}  {}", m.iid, m.title)).detail(summary(m))
            }),
        )),
    }
    column(parts).spacing(6.0)
}

fn summary(m: &MergeRequest) -> String {
    let mut said = vec![
        m.author.username.clone(),
        format!("{} → {}", m.source_branch, m.target_branch),
        day(&m.updated_at).to_string(),
    ];
    if m.state != "opened" {
        said.push(filter_label(&m.state));
    }
    if m.has_conflicts {
        said.push(t!("gl.conflicts").to_string());
    }
    if m.user_notes_count > 0 {
        said.push(format!("💬 {}", m.user_notes_count));
    }
    if let Some(p) = &m.head_pipeline {
        said.push(format!("{} {}", mark(&p.status), p.status));
    }
    said.join(" · ")
}

fn new_request(state: &State, new: &NewMergeRequest) -> Node {
    let names: Vec<String> = state.branches.iter().map(|b| b.name.clone()).collect();
    let pick = |id: &str, value: &str, label: String| {
        row([
            muted(label),
            select(
                id,
                names.clone(),
                Some(value.to_string()).filter(|v| !v.is_empty()),
            ),
        ])
        .spacing(6.0)
    };
    let ready = !state.busy
        && !new.source.is_empty()
        && !new.target.is_empty()
        && !new.title.trim().is_empty()
        && new.source != new.target;
    scroll(
        column([
            heading(t!("gl.mr_new").to_string()),
            pick("new_source", &new.source, t!("gl.mr_source").to_string()),
            pick("new_target", &new.target, t!("gl.mr_target").to_string()),
            muted(t!("gl.mr_pushed_hint").to_string()),
            input("new_title", new.title.clone(), t!("gl.title").to_string()),
            editor("new_description", new.description.clone(), "md")
                .placeholder(t!("gl.description").to_string())
                .height(120.0),
            row([
                checkbox("new_draft", t!("gl.mr_draft").to_string(), new.draft),
                checkbox(
                    "new_remove",
                    t!("gl.mr_remove_source").to_string(),
                    new.remove_source,
                ),
                checkbox("new_squash", t!("gl.mr_squash").to_string(), new.squash),
            ])
            .spacing(10.0),
            row([
                primary(t!("gl.create").to_string(), "mr:create", ready),
                tool(t!("gl.cancel").to_string(), "mr:cancel", !state.busy),
            ])
            .spacing(6.0),
        ])
        .spacing(6.0),
    )
}

fn one_request(state: &State, shown: &Shown) -> Node {
    let m = &shown.request;
    let free = !state.busy;
    let open = m.state == "opened";
    let mut parts = vec![
        row([
            tool(t!("gl.back").to_string(), "mr:back", true),
            heading(format!("!{}  {}", m.iid, m.title)),
        ])
        .spacing(8.0),
        muted(summary(m)),
    ];
    if let Some(status) = m.detailed_merge_status.as_deref().filter(|_| open) {
        parts.push(muted(t!("gl.merge_status", status = status).to_string()));
    }
    if let Some(said) = m.description.as_deref().filter(|d| !d.trim().is_empty()) {
        parts.push(text(said.to_string()));
    }

    let mut actions = vec![
        tool(t!("gl.copy_link").to_string(), "mr:link", true),
        tool(
            t!("gl.mr_changes").to_string(),
            "mr:changes",
            shown.files.as_ref().is_some_and(|f| !f.is_empty()),
        ),
    ];
    if open {
        actions.push(tool(t!("gl.approve").to_string(), "mr:approve", free));
        actions.push(tool(t!("gl.unapprove").to_string(), "mr:unapprove", free));
        actions.push(armed(
            state,
            t!("gl.mr_close").to_string(),
            "confirm:mr:close".into(),
        ));
    } else if m.state == "closed" {
        actions.push(tool(t!("gl.mr_reopen").to_string(), "mr:reopen", free));
    }
    actions.push(armed(
        state,
        t!("gl.mr_delete").to_string(),
        "confirm:mr:delete".into(),
    ));
    parts.push(row(actions).spacing(6.0));

    if open {
        let mut line = vec![
            checkbox(
                "merge_squash",
                t!("gl.mr_squash").to_string(),
                shown.merge.squash,
            ),
            checkbox(
                "merge_remove",
                t!("gl.mr_remove_source").to_string(),
                shown.merge.remove_source,
            ),
        ];
        if m.head_pipeline.as_ref().is_some_and(Pipeline::running) {
            line.push(checkbox(
                "merge_later",
                t!("gl.merge_later").to_string(),
                shown.merge.when_pipeline_succeeds,
            ));
        }
        if m.sha.is_some() && !m.has_conflicts && !m.draft {
            line.push(armed(
                state,
                t!("gl.merge").to_string(),
                "confirm:mr:merge".into(),
            ));
        } else if m.draft {
            line.push(muted(t!("gl.merge_draft").to_string()));
        } else if m.has_conflicts {
            line.push(text(t!("gl.conflicts").to_string()).style(TextStyle::Error));
        }
        parts.push(row(line).spacing(10.0));
    }

    parts.push(separator());
    parts.push(heading(t!("gl.pipeline").to_string()));
    match shown.pipelines.first() {
        None => parts.push(muted(t!("gl.pipeline_none").to_string())),
        Some(p) => parts.push(
            row([
                text(format!("{} #{} {}", mark(&p.status), p.id, p.status)),
                tool(
                    t!("gl.jobs").to_string(),
                    format!("pipeline:show:{}", p.id),
                    true,
                ),
            ])
            .spacing(8.0),
        ),
    }

    parts.push(separator());
    match &shown.files {
        None => parts.push(muted(t!("gl.loading").to_string())),
        Some(files) => {
            parts.push(heading(t!("gl.files", n = files.len()).to_string()));
            parts.push(list(
                "mr_files",
                files.iter().enumerate().map(|(at, f)| {
                    let kind = if f.new_file {
                        "A"
                    } else if f.deleted_file {
                        "D"
                    } else if f.renamed_file {
                        "R"
                    } else {
                        "M"
                    };
                    let mut detail = kind.to_string();
                    if f.too_large {
                        detail = format!("{kind} · {}", t!("gl.too_large"));
                    }
                    item(at.to_string(), f.new_path.clone()).detail(detail)
                }),
            ));
        }
    }

    parts.push(separator());
    parts.push(heading(t!("gl.comments").to_string()));
    match &shown.discussions {
        None => parts.push(muted(t!("gl.loading").to_string())),
        Some(all) => {
            let said: Vec<&Discussion> = all.iter().filter(|d| d.said()).collect();
            if said.is_empty() {
                parts.push(muted(t!("gl.comments_none").to_string()));
            }
            for d in said {
                thread(state, shown, d, &mut parts);
            }
        }
    }
    parts.push(
        editor("comment", shown.comment.clone(), "md")
            .placeholder(t!("gl.comment_new").to_string())
            .height(80.0),
    );
    parts.push(row([primary(
        t!("gl.comment_send").to_string(),
        "comment:send",
        free,
    )]));
    scroll(column(parts).spacing(6.0))
}

fn thread(state: &State, shown: &Shown, d: &Discussion, parts: &mut Vec<Node>) {
    for (at, note) in d.notes.iter().filter(|n| !n.system).enumerate() {
        let mut who = format!("{} · {}", note.author.username, day(&note.created_at));
        if at > 0 {
            who = format!("    ↳ {who}");
        }
        if let Some(position) = &note.position {
            let path = position.new_path.as_ref().or(position.old_path.as_ref());
            if let (0, Some(path)) = (at, path) {
                let line = position.new_line.or(position.old_line);
                who.push_str(&format!(
                    " · {path}{}",
                    line.map_or_else(String::new, |l| format!(":{l}"))
                ));
            }
        }
        parts.push(muted(who));
        parts.push(text(note.body.clone()));
    }
    let mut tools = vec![tool(
        t!("gl.reply").to_string(),
        format!("reply:open:{}", d.id),
        true,
    )];
    if d.resolvable() {
        let resolved = d.resolved();
        tools.push(tool(
            if resolved {
                t!("gl.unresolve").to_string()
            } else {
                t!("gl.resolve").to_string()
            },
            format!(
                "{}:{}",
                if resolved { "unresolve" } else { "resolve" },
                d.id
            ),
            !state.busy,
        ));
        if resolved {
            tools.push(muted(t!("gl.resolved").to_string()));
        }
    }
    parts.push(row(tools).spacing(6.0));
    if let Some((id, answer)) = &shown.reply {
        if *id == d.id {
            parts.push(
                row([
                    input("reply", answer.clone(), t!("gl.reply").to_string())
                        .on_submit("reply:send"),
                    primary(t!("gl.send").to_string(), "reply:send", !state.busy),
                ])
                .spacing(6.0),
            );
        }
    }
}

// ─── Pipelines ──────────────────────────────────────────────────────────────

fn pipelines(state: &State) -> Node {
    let free = !state.busy;
    let mut parts = vec![row([
        input("run_ref", state.run_ref.clone(), t!("gl.ref").to_string()).on_submit("pipeline:run"),
        tool(
            t!("gl.pipeline_run").to_string(),
            "pipeline:run",
            free && !state.run_ref.trim().is_empty(),
        ),
    ])
    .spacing(6.0)];
    if let Some((id, jobs)) = &state.pipeline {
        parts.push(
            row([
                tool(t!("gl.back").to_string(), "pipeline:back", true),
                heading(format!("#{id}")),
                tool(
                    t!("gl.retry").to_string(),
                    format!("pipeline:retry:{id}"),
                    free,
                ),
                tool(
                    t!("gl.stop").to_string(),
                    format!("pipeline:cancel:{id}"),
                    free,
                ),
                armed(
                    state,
                    t!("gl.delete").to_string(),
                    format!("confirm:pipeline:delete:{id}"),
                ),
            ])
            .spacing(6.0),
        );
        match jobs {
            None => parts.push(muted(t!("gl.loading").to_string())),
            Some(jobs) => parts.extend(jobs.iter().map(|j| job(state, j))),
        }
        return scroll(column(parts).spacing(6.0));
    }
    match &state.pipelines {
        None => parts.push(muted(t!("gl.loading").to_string())),
        Some(found) if found.is_empty() => parts.push(muted(t!("gl.pipeline_none").to_string())),
        Some(found) => parts.push(list(
            "pipelines",
            found.iter().map(|p| {
                item(
                    p.id.to_string(),
                    format!("{} #{}  {}", mark(&p.status), p.id, p.reference),
                )
                .detail(format!(
                    "{} · {} · {}",
                    p.status,
                    p.sha.get(..8).unwrap_or(&p.sha),
                    day(&p.created_at)
                ))
            }),
        )),
    }
    column(parts).spacing(6.0)
}

fn job(state: &State, j: &Job) -> Node {
    let mut line = vec![text(format!(
        "{} {} / {}  {}{}",
        mark(&j.status),
        j.stage,
        j.name,
        j.status,
        j.duration
            .map_or_else(String::new, |d| format!(" · {} s", d.round()))
    ))];
    line.push(tool(
        t!("gl.log").to_string(),
        format!("job:log:{}", j.id),
        true,
    ));
    let action = match j.status.as_str() {
        "manual" => Some(("play", t!("gl.play"))),
        "running" | "pending" | "created" => Some(("cancel", t!("gl.stop"))),
        "failed" | "canceled" | "success" => Some(("retry", t!("gl.retry"))),
        _ => None,
    };
    if let Some((verb, label)) = action {
        // A job started by hand often deploys: it is confirmed.
        if verb == "play" {
            line.push(armed(
                state,
                label.to_string(),
                format!("confirm:job:play:{}", j.id),
            ));
        } else {
            line.push(tool(
                label.to_string(),
                format!("job:{verb}:{}", j.id),
                !state.busy,
            ));
        }
    }
    row(line).spacing(8.0)
}

// ─── Tags ───────────────────────────────────────────────────────────────────

fn tags(state: &State) -> Node {
    let mut parts = vec![row([
        input(
            "tag_name",
            state.tag_name.clone(),
            t!("gl.tag_name").to_string(),
        ),
        input("tag_ref", state.tag_ref.clone(), t!("gl.ref").to_string()),
        input(
            "tag_message",
            state.tag_message.clone(),
            t!("gl.tag_message").to_string(),
        ),
        primary(t!("gl.tag_create").to_string(), "tag:create", !state.busy),
    ])
    .spacing(6.0)];
    match &state.tags {
        None => parts.push(muted(t!("gl.loading").to_string())),
        Some(found) if found.is_empty() => parts.push(muted(t!("gl.tags_none").to_string())),
        Some(found) => {
            for tag in found {
                let mut line = vec![
                    text(tag.name.clone()).style(TextStyle::Bold),
                    muted(format!("{}  {}", tag.commit.short_id, tag.commit.title)),
                ];
                if tag.protected {
                    line.push(muted(t!("gl.protected").to_string()));
                }
                line.push(armed(
                    state,
                    t!("gl.delete").to_string(),
                    format!("confirm:tag:delete:{}", tag.name),
                ));
                parts.push(row(line).spacing(8.0));
                if let Some(message) = tag.message.as_deref().filter(|m| !m.trim().is_empty()) {
                    parts.push(muted(message.trim().to_string()));
                }
            }
        }
    }
    scroll(column(parts).spacing(4.0))
}

// ─── Settings ───────────────────────────────────────────────────────────────

pub fn settings(state: &State, hosts: &[(String, String)]) -> Node {
    let mut parts = vec![muted(t!("gl.settings_help").to_string())];
    if hosts.is_empty() {
        parts.push(muted(t!("gl.settings_none").to_string()));
    }
    for (host, address) in hosts {
        parts.push(
            row([
                text(host.clone()).style(TextStyle::Bold),
                muted(address.clone()),
                armed(
                    state,
                    t!("gl.forget").to_string(),
                    format!("confirm:forget:{host}"),
                ),
            ])
            .spacing(8.0),
        );
    }
    column(parts).spacing(8.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Note, Tag, User};
    use crate::remote::{Checkout, Project};

    fn ready() -> State {
        State {
            checkout: Some(Checkout {
                project: Project {
                    host: "gitlab.com".into(),
                    path: "a/b".into(),
                },
                branch: Some("topic".into()),
            }),
            link: Link::Ready(User::default()),
            ..State::default()
        }
    }

    fn drawn(state: &State) -> String {
        serde_json::to_string(&panel(state)).unwrap()
    }

    fn request() -> MergeRequest {
        MergeRequest {
            iid: 4,
            title: "A change".into(),
            state: "opened".into(),
            sha: Some("abc".into()),
            ..MergeRequest::default()
        }
    }

    #[test]
    fn without_a_project_or_a_token_the_panel_says_what_to_do() {
        assert!(drawn(&State::default()).contains(&t!("gl.no_project").to_string()));
        let mut state = ready();
        state.link = Link::NoToken;
        let said = drawn(&state);
        assert!(said.contains("\"connect\"") && said.contains("gitlab.com"));
        assert!(said.contains("\"secret\":true"), "the token is hidden");
    }

    #[test]
    fn a_merge_request_offers_its_merge_only_when_it_can_be_merged() {
        let mut state = ready();
        state.shown = Some(Shown {
            request: request(),
            ..Shown::default()
        });
        assert!(drawn(&state).contains("confirm:mr:merge"));
        for change in [
            (|m: &mut MergeRequest| m.draft = true) as fn(&mut MergeRequest),
            |m| m.has_conflicts = true,
            |m| m.state = "merged".into(),
            |m| m.sha = None,
        ] {
            let mut other = state.shown.clone().unwrap();
            change(&mut other.request);
            let mut s = ready();
            s.shown = Some(other);
            assert!(!drawn(&s).contains("confirm:mr:merge"));
        }
        assert!(drawn(&state).contains("confirm:mr:delete"));
    }

    #[test]
    fn the_comments_show_who_said_what_and_where() {
        let mut state = ready();
        let note = |body: &str, system| Note {
            body: body.into(),
            system,
            resolvable: !system,
            author: User {
                username: "ada".into(),
                name: "Ada".into(),
            },
            position: Some(crate::api::Position {
                new_path: Some("src/a.rs".into()),
                new_line: Some(12),
                ..Default::default()
            }),
            ..Note::default()
        };
        state.shown = Some(Shown {
            request: request(),
            discussions: Some(vec![
                Discussion {
                    id: "d1".into(),
                    notes: vec![note("rename this", false)],
                },
                Discussion {
                    id: "d2".into(),
                    notes: vec![note("added 1 commit", true)],
                },
            ]),
            ..Shown::default()
        });
        let said = drawn(&state);
        assert!(said.contains("rename this") && said.contains("src/a.rs:12"));
        assert!(said.contains("resolve:d1"));
        assert!(
            !said.contains("added 1 commit"),
            "GitLab's own lines are left"
        );
    }

    #[test]
    fn a_manual_job_and_a_tag_to_delete_ask_twice() {
        let mut state = ready();
        state.tab = Tab::Pipelines;
        state.pipeline = Some((
            3,
            Some(vec![Job {
                id: 8,
                status: "manual".into(),
                ..Job::default()
            }]),
        ));
        assert!(drawn(&state).contains("confirm:job:play:8"));
        state.tab = Tab::Tags;
        state.tags = Some(vec![Tag {
            name: "v1".into(),
            ..Tag::default()
        }]);
        assert!(drawn(&state).contains("confirm:tag:delete:v1"));
    }
}
