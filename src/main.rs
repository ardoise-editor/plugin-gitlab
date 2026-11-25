//! The GitLab plugin of Ardoise: the merge requests of the project (list,
//! make, read with their files, diffs and comments, approve, merge, close,
//! delete), its pipelines and jobs, its tags. It shows as a tab of the Git
//! panel and works with or without the Git plugin.

#[macro_use]
extern crate rust_i18n;

i18n!("locales", fallback = "en");

mod actions;
mod api;
mod fsutil;
mod remote;
mod state;
mod store;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use ardoise_plugin::protocol::{Context, InitializeParams, Settings, UiEvent};
use ardoise_plugin::{run, Host, Plugin};

use api::{Client, Error};
use state::{Link, State, Tab, FILTERS};
use store::Store;

pub const PANEL: &str = "main";
pub const SETTINGS: &str = "settings";

/// How often a pipeline that runs is looked at while the panel is open.
const TICK: Duration = Duration::from_secs(15);

fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// The state, and how to talk to the editor, for the work done in the
/// background.
#[derive(Clone)]
pub struct Ctx {
    pub shared: Arc<Mutex<State>>,
    pub store: Arc<Mutex<Store>>,
    pub host: Host,
    pub open: Arc<AtomicBool>,
    /// Changes with the project: an answer asked for another one is dropped.
    pub generation: Arc<AtomicU64>,
}

impl Ctx {
    pub fn with<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut lock(&self.shared))
    }

    pub fn now(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Changes the state, if the project is still the one of `generation`.
    pub fn answer(&self, generation: u64, f: impl FnOnce(&mut State)) {
        let mut state = lock(&self.shared);
        if self.now() == generation {
            f(&mut state);
        }
    }

    pub fn render(&self) {
        let state = lock(&self.shared);
        self.host.render(PANEL, ui::panel(&state));
    }

    pub fn render_settings(&self) {
        let hosts: Vec<(String, String)> = {
            let store = lock(&self.store);
            store
                .servers
                .keys()
                .map(|h| (h.clone(), store.address(h)))
                .collect()
        };
        let state = lock(&self.shared);
        self.host.render(SETTINGS, ui::settings(&state, &hosts));
    }

    /// The client of the project on show, when its server has a token.
    pub async fn client(&self) -> Option<Result<Client, Error>> {
        let project = self.with(|s| s.checkout.as_ref().map(|c| c.project.clone()))?;
        let store = self.store.clone();
        let host = project.host.clone();
        // The keychain may take a while: away from the other tasks.
        let (address, token) = tokio::task::spawn_blocking(move || {
            let store = lock(&store);
            (store.address(&host), store.token(&host))
        })
        .await
        .ok()?;
        (!token.is_empty()).then(|| Client::new(&address, &project.path, &token))
    }

    /// Tries the token of the server, then reads what is on show.
    pub async fn connect(&self) {
        let generation = self.now();
        let link = match self.client().await {
            None => Link::NoToken,
            Some(Err(error)) => Link::Failed(error.to_string()),
            Some(Ok(client)) => match client.me().await {
                Ok(user) => Link::Ready(user),
                Err(error) => Link::Failed(error.to_string()),
            },
        };
        let ready = matches!(link, Link::Ready(_));
        self.answer(generation, |s| s.link = link);
        self.render();
        if ready {
            self.load().await;
        }
    }

    /// Reads again what the tab shows.
    pub async fn load(&self) {
        let generation = self.now();
        let Some(Ok(client)) = self.client().await else {
            return;
        };
        let result = actions::read(self, &client, generation).await;
        if let Err(error) = result {
            self.answer(generation, |s| match error {
                Error::Unauthorized => s.link = Link::Failed(error.to_string()),
                other => s.notice = Some((other.to_string(), true)),
            });
        }
        self.render();
    }

    /// Looks for the project of a file, else of the first project open. A
    /// file outside any project leaves the one on show.
    pub async fn follow(&self, file: Option<PathBuf>) {
        let workspace = self.with(|s| s.workspace.clone());
        let found = tokio::task::spawn_blocking(move || {
            file.iter()
                .chain(workspace.iter())
                .find_map(|path| remote::detect(path))
        })
        .await
        .ok()
        .flatten();
        let Some(found) = found else {
            return;
        };
        let changed = {
            let mut state = lock(&self.shared);
            let same = state
                .checkout
                .as_ref()
                .is_some_and(|c| c.project == found.project);
            if !same {
                self.generation.fetch_add(1, Ordering::SeqCst);
                state.reset();
            }
            state.checkout = Some(found);
            !same
        };
        self.render();
        if changed && self.open.load(Ordering::SeqCst) {
            self.connect().await;
        }
    }
}

struct GitLab {
    host: Option<Host>,
    shared: Arc<Mutex<State>>,
    store: Arc<Mutex<Store>>,
    open: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    runtime: tokio::runtime::Runtime,
}

impl GitLab {
    fn ctx(&self) -> Option<Ctx> {
        Some(Ctx {
            shared: self.shared.clone(),
            store: self.store.clone(),
            host: self.host.clone()?,
            open: self.open.clone(),
            generation: self.generation.clone(),
        })
    }

    fn follow(&self, file: Option<PathBuf>) {
        if let Some(ctx) = self.ctx() {
            self.runtime.spawn(async move { ctx.follow(file).await });
        }
    }

    /// The panel is on show: connected if it is not, else read again.
    fn shown(&self) {
        let Some(ctx) = self.ctx() else {
            return;
        };
        self.runtime.spawn(async move {
            let file = ctx.with(|s| s.file.clone());
            ctx.follow(file).await;
            match ctx.with(|s| s.link.clone()) {
                Link::Ready(_) => ctx.load().await,
                _ => ctx.connect().await,
            }
        });
    }
}

impl Plugin for GitLab {
    fn initialize(&mut self, host: &Host, params: &InitializeParams) {
        rust_i18n::set_locale(&params.settings.locale);
        fsutil::set_data_dir(params.data_dir.clone());
        self.host = Some(host.clone());
        *lock(&self.store) = Store::load();
        lock(&self.shared).workspace = params.workspace.clone();
        let Some(ctx) = self.ctx() else {
            return;
        };
        self.runtime.spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                let wanted = ctx.open.load(Ordering::SeqCst)
                    && ctx.with(|s| !s.busy && s.pipeline_running());
                if wanted {
                    ctx.load().await;
                }
            }
        });
    }

    fn command(&mut self, host: &Host, command: &str, context: &Context) {
        if !context.workspace.is_empty() {
            lock(&self.shared).workspace = context.workspace.clone();
        }
        let tab = match command {
            "gitlab.pipelines" => Tab::Pipelines,
            "gitlab.tags" => Tab::Tags,
            _ => Tab::MergeRequests,
        };
        {
            let mut state = lock(&self.shared);
            state.tab = tab;
            if context.file.is_some() {
                state.file = context.file.clone();
            }
            if command == "gitlab.mr_new" {
                let draft = state.draft();
                state.shown = None;
                state.new = Some(draft);
            }
        }
        host.show_panel(PANEL);
        self.open.store(true, Ordering::SeqCst);
        self.shown();
    }

    fn panel_opened(&mut self, _: &Host, panel: &str) {
        match panel {
            PANEL => {
                self.open.store(true, Ordering::SeqCst);
                if let Some(ctx) = self.ctx() {
                    ctx.render();
                }
                self.shown();
            }
            SETTINGS => {
                if let Some(ctx) = self.ctx() {
                    ctx.render_settings();
                }
            }
            _ => {}
        }
    }

    fn panel_closed(&mut self, _: &Host, panel: &str) {
        if panel == PANEL {
            self.open.store(false, Ordering::SeqCst);
        }
    }

    fn ui_event(&mut self, _: &Host, event: &UiEvent) {
        let Some(ctx) = self.ctx() else {
            return;
        };
        let _guard = self.runtime.enter();
        actions::event(&ctx, event);
    }

    fn active_file_changed(&mut self, _: &Host, path: Option<&Path>) {
        let Some(path) = path else {
            return;
        };
        lock(&self.shared).file = Some(path.to_path_buf());
        self.follow(Some(path.to_path_buf()));
    }

    fn settings_changed(&mut self, _: &Host, settings: &Settings) {
        rust_i18n::set_locale(&settings.locale);
        if let Some(ctx) = self.ctx() {
            ctx.render();
        }
    }
}

fn main() -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    // The filter of the list starts on the merge requests still open.
    debug_assert_eq!(FILTERS[0], "opened");
    run(GitLab {
        host: None,
        shared: Arc::default(),
        store: Arc::default(),
        open: Arc::default(),
        generation: Arc::default(),
        runtime,
    })
}
