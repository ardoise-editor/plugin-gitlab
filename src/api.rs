//! The REST API of GitLab (v4): merge requests, their files and comments,
//! pipelines and tags.

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

/// The most pages of a list that are read (100 a page).
const MAX_PAGES: u32 = 10;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The token is missing, wrong or expired.
    Unauthorized,
    /// What GitLab answered, with its status.
    Server(u16, String),
    /// The server was not reached, or its answer not read.
    Network(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unauthorized => write!(f, "{}", t!("gl.err_token")),
            Error::Server(status, said) => write!(f, "GitLab {status}: {said}"),
            Error::Network(why) => write!(f, "{}", t!("gl.err_network", why = why.as_str())),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct User {
    pub username: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Pipeline {
    pub id: u64,
    pub status: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub sha: String,
    pub web_url: String,
    pub created_at: String,
}

impl Pipeline {
    /// It can still change by itself.
    pub fn running(&self) -> bool {
        matches!(
            self.status.as_str(),
            "created" | "waiting_for_resource" | "preparing" | "pending" | "running"
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub stage: String,
    pub status: String,
    pub duration: Option<f64>,
    pub allow_failure: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct MergeRequest {
    pub iid: u64,
    pub title: String,
    pub description: Option<String>,
    /// `opened`, `closed`, `merged` or `locked`.
    pub state: String,
    pub draft: bool,
    pub source_branch: String,
    pub target_branch: String,
    pub author: User,
    pub web_url: String,
    pub updated_at: String,
    /// The commit at the head of the source: a merge asks for this one.
    pub sha: Option<String>,
    pub has_conflicts: bool,
    /// Why it can or can't be merged (`mergeable`, `ci_must_pass`…).
    pub detailed_merge_status: Option<String>,
    pub user_notes_count: u64,
    pub head_pipeline: Option<Pipeline>,
}

/// A file of a merge request.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct FileDiff {
    pub old_path: String,
    pub new_path: String,
    /// The hunks, without the lines that name the file.
    pub diff: String,
    pub new_file: bool,
    pub renamed_file: bool,
    pub deleted_file: bool,
    /// The server left the diff out: it is too big.
    pub too_large: bool,
}

impl FileDiff {
    /// The diff as `git diff` writes it, which is what the editor draws.
    pub fn patch(&self) -> String {
        let (old, new) = (&self.old_path, &self.new_path);
        let mut out = format!("diff --git a/{old} b/{new}\n");
        if self.new_file {
            out.push_str("new file mode 100644\n");
        }
        if self.deleted_file {
            out.push_str("deleted file mode 100644\n");
        }
        if self.renamed_file {
            out.push_str(&format!("rename from {old}\nrename to {new}\n"));
        }
        if !self.diff.is_empty() {
            let from = if self.new_file {
                "/dev/null".to_string()
            } else {
                format!("a/{old}")
            };
            let to = if self.deleted_file {
                "/dev/null".to_string()
            } else {
                format!("b/{new}")
            };
            out.push_str(&format!("--- {from}\n+++ {to}\n{}", self.diff));
            if !self.diff.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Position {
    pub new_path: Option<String>,
    pub old_path: Option<String>,
    pub new_line: Option<u64>,
    pub old_line: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Note {
    pub id: u64,
    pub body: String,
    pub author: User,
    pub created_at: String,
    /// Said by GitLab itself ("added 1 commit").
    pub system: bool,
    pub resolvable: bool,
    pub resolved: bool,
    /// The line of the diff it is about, if any.
    pub position: Option<Position>,
}

/// A thread of comments.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Discussion {
    pub id: String,
    pub notes: Vec<Note>,
}

impl Discussion {
    /// What people said (not GitLab's own lines).
    pub fn said(&self) -> bool {
        self.notes.iter().any(|n| !n.system)
    }
    pub fn resolvable(&self) -> bool {
        self.notes.iter().any(|n| n.resolvable)
    }
    pub fn resolved(&self) -> bool {
        self.resolvable() && self.notes.iter().all(|n| !n.resolvable || n.resolved)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Commit {
    pub short_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Tag {
    pub name: String,
    pub message: Option<String>,
    pub protected: bool,
    pub commit: Commit,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Branch {
    pub name: String,
    pub default: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ProjectInfo {
    pub default_branch: Option<String>,
    pub squash_option: Option<String>,
}

/// A merge request to make.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewMergeRequest {
    pub source: String,
    pub target: String,
    pub title: String,
    pub description: String,
    pub draft: bool,
    pub remove_source: bool,
    pub squash: bool,
}

/// How a merge request is merged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeOptions {
    pub squash: bool,
    pub remove_source: bool,
    /// Merged by the server once the pipeline passes.
    pub when_pipeline_succeeds: bool,
}

/// `group/name` as one part of an address.
pub fn encode(part: &str) -> String {
    let mut out = String::new();
    for b in part.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What GitLab says went wrong, out of the ways it says it.
fn message_of(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return body.chars().take(300).collect();
    };
    fn flat(value: &Value) -> String {
        match value {
            Value::String(s) => s.clone(),
            Value::Array(items) => items.iter().map(flat).collect::<Vec<_>>().join(", "),
            Value::Object(map) => map
                .iter()
                .map(|(k, v)| format!("{k}: {}", flat(v)))
                .collect::<Vec<_>>()
                .join("; "),
            other => other.to_string(),
        }
    }
    ["message", "error_description", "error"]
        .iter()
        .find_map(|k| value.get(k))
        .map_or_else(|| body.chars().take(300).collect(), flat)
}

/// The API of one project, with the token of its server.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    /// `https://host/api/v4`.
    base: String,
    token: String,
    project: String,
}

impl Client {
    /// `server`: the address of the web interface (`https://gitlab.com`). Only
    /// `https` carries a token, but to this machine itself.
    pub fn new(server: &str, project: &str, token: &str) -> Result<Client, Error> {
        let server = server.trim_end_matches('/');
        let local = ["http://localhost", "http://127.0.0.1"].iter().any(|l| {
            server
                .strip_prefix(l)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
        });
        if !server.starts_with("https://") && !local {
            return Err(Error::Network(t!("gl.err_https").to_string()));
        }
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent("ardoise-gitlab")
            // An answer that sends elsewhere is not followed: the token
            // would go with it.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Client {
            http,
            base: format!("{server}/api/v4"),
            token: token.trim().to_string(),
            project: encode(project),
        })
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(String, Option<u32>), Error> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|e| {
            // The address, which a message could repeat, holds no secret.
            Error::Network(e.without_url().to_string())
        })?;
        let status = response.status();
        let next = response
            .headers()
            .get("x-next-page")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let text = response
            .text()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        if status == StatusCode::UNAUTHORIZED {
            return Err(Error::Unauthorized);
        }
        if !status.is_success() {
            return Err(Error::Server(status.as_u16(), message_of(&text)));
        }
        Ok((text, next))
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T, Error> {
        let (text, _) = self.send(method, path, body).await?;
        serde_json::from_str(&text).map_err(|e| Error::Network(e.to_string()))
    }

    /// A call whose answer is not needed.
    async fn act(&self, method: Method, path: &str, body: Option<Value>) -> Result<(), Error> {
        self.send(method, path, body).await.map(|_| ())
    }

    /// Every page of a list. `path` already has a `?`.
    async fn list<T: DeserializeOwned>(&self, path: &str, pages: u32) -> Result<Vec<T>, Error> {
        let mut all = Vec::new();
        let mut page = 1;
        for _ in 0..pages {
            let (text, next) = self
                .send(
                    Method::GET,
                    &format!("{path}&per_page=100&page={page}"),
                    None,
                )
                .await?;
            let mut found: Vec<T> =
                serde_json::from_str(&text).map_err(|e| Error::Network(e.to_string()))?;
            all.append(&mut found);
            match next {
                Some(next) => page = next,
                None => break,
            }
        }
        Ok(all)
    }

    fn mr(&self, iid: u64) -> String {
        format!("/projects/{}/merge_requests/{iid}", self.project)
    }

    fn at(&self, rest: &str) -> String {
        format!("/projects/{}{rest}", self.project)
    }

    pub async fn me(&self) -> Result<User, Error> {
        self.call(Method::GET, "/user", None).await
    }

    pub async fn project(&self) -> Result<ProjectInfo, Error> {
        self.call(Method::GET, &self.at(""), None).await
    }

    pub async fn branches(&self) -> Result<Vec<Branch>, Error> {
        self.list(&self.at("/repository/branches?"), MAX_PAGES)
            .await
    }

    // ─── Merge requests ─────────────────────────────────────────────────

    /// `state`: `opened`, `merged`, `closed` or `all`, latest changed first.
    pub async fn merge_requests(&self, state: &str) -> Result<Vec<MergeRequest>, Error> {
        let path = self.at(&format!(
            "/merge_requests?state={}&order_by=updated_at",
            encode(state)
        ));
        self.list(&path, 2).await
    }

    pub async fn merge_request(&self, iid: u64) -> Result<MergeRequest, Error> {
        self.call(Method::GET, &self.mr(iid), None).await
    }

    pub async fn create(&self, new: &NewMergeRequest) -> Result<MergeRequest, Error> {
        let title = if new.draft {
            format!("Draft: {}", new.title.trim())
        } else {
            new.title.trim().to_string()
        };
        let body = json!({
            "source_branch": new.source.trim(),
            "target_branch": new.target.trim(),
            "title": title,
            "description": new.description,
            "remove_source_branch": new.remove_source,
            "squash": new.squash,
        });
        self.call(Method::POST, &self.at("/merge_requests"), Some(body))
            .await
    }

    /// Removes a merge request for good (GitLab lets only owners and
    /// administrators: the others close it).
    pub async fn delete(&self, iid: u64) -> Result<(), Error> {
        self.act(Method::DELETE, &self.mr(iid), None).await
    }

    pub async fn set_open(&self, iid: u64, open: bool) -> Result<(), Error> {
        let event = if open { "reopen" } else { "close" };
        self.act(
            Method::PUT,
            &self.mr(iid),
            Some(json!({"state_event": event})),
        )
        .await
    }

    /// Merges. `sha`: the head that was looked at; GitLab refuses if the
    /// branch moved since, so nothing unseen is merged.
    pub async fn merge(&self, iid: u64, sha: &str, how: &MergeOptions) -> Result<(), Error> {
        let mut body = json!({
            "sha": sha,
            "squash": how.squash,
            "should_remove_source_branch": how.remove_source,
        });
        if how.when_pipeline_succeeds {
            body["merge_when_pipeline_succeeds"] = json!(true);
        }
        self.act(Method::PUT, &format!("{}/merge", self.mr(iid)), Some(body))
            .await
    }

    pub async fn approve(&self, iid: u64, sha: &str) -> Result<(), Error> {
        self.act(
            Method::POST,
            &format!("{}/approve", self.mr(iid)),
            Some(json!({ "sha": sha })),
        )
        .await
    }

    pub async fn unapprove(&self, iid: u64) -> Result<(), Error> {
        self.act(Method::POST, &format!("{}/unapprove", self.mr(iid)), None)
            .await
    }

    pub async fn diffs(&self, iid: u64) -> Result<Vec<FileDiff>, Error> {
        self.list(&format!("{}/diffs?", self.mr(iid)), MAX_PAGES)
            .await
    }

    pub async fn discussions(&self, iid: u64) -> Result<Vec<Discussion>, Error> {
        self.list(&format!("{}/discussions?", self.mr(iid)), MAX_PAGES)
            .await
    }

    /// A new thread on the merge request.
    pub async fn comment(&self, iid: u64, body: &str) -> Result<(), Error> {
        self.act(
            Method::POST,
            &format!("{}/discussions", self.mr(iid)),
            Some(json!({ "body": body })),
        )
        .await
    }

    pub async fn reply(&self, iid: u64, discussion: &str, body: &str) -> Result<(), Error> {
        let path = format!("{}/discussions/{}/notes", self.mr(iid), encode(discussion));
        self.act(Method::POST, &path, Some(json!({ "body": body })))
            .await
    }

    pub async fn resolve(&self, iid: u64, discussion: &str, resolved: bool) -> Result<(), Error> {
        let path = format!(
            "{}/discussions/{}?resolved={resolved}",
            self.mr(iid),
            encode(discussion)
        );
        self.act(Method::PUT, &path, None).await
    }

    pub async fn mr_pipelines(&self, iid: u64) -> Result<Vec<Pipeline>, Error> {
        self.list(&format!("{}/pipelines?", self.mr(iid)), 1).await
    }

    // ─── Pipelines ──────────────────────────────────────────────────────

    /// The latest pipelines.
    pub async fn pipelines(&self) -> Result<Vec<Pipeline>, Error> {
        self.list(&self.at("/pipelines?order_by=id"), 1).await
    }

    pub async fn jobs(&self, pipeline: u64) -> Result<Vec<Job>, Error> {
        self.list(&self.at(&format!("/pipelines/{pipeline}/jobs?")), MAX_PAGES)
            .await
    }

    /// Runs a new pipeline on a branch or a tag.
    pub async fn run_pipeline(&self, reference: &str) -> Result<(), Error> {
        self.act(
            Method::POST,
            &self.at("/pipeline"),
            Some(json!({ "ref": reference.trim() })),
        )
        .await
    }

    /// `retry` (the jobs that failed) or `cancel`.
    pub async fn pipeline_action(&self, pipeline: u64, action: &str) -> Result<(), Error> {
        let path = self.at(&format!("/pipelines/{pipeline}/{}", encode(action)));
        self.act(Method::POST, &path, None).await
    }

    pub async fn delete_pipeline(&self, pipeline: u64) -> Result<(), Error> {
        self.act(
            Method::DELETE,
            &self.at(&format!("/pipelines/{pipeline}")),
            None,
        )
        .await
    }

    /// `retry`, `cancel` or `play` (a manual job).
    pub async fn job_action(&self, job: u64, action: &str) -> Result<(), Error> {
        let path = self.at(&format!("/jobs/{job}/{}", encode(action)));
        self.act(Method::POST, &path, None).await
    }

    /// What a job wrote.
    pub async fn job_log(&self, job: u64) -> Result<String, Error> {
        let path = self.at(&format!("/jobs/{job}/trace"));
        self.send(Method::GET, &path, None)
            .await
            .map(|(text, _)| text)
    }

    // ─── Tags ───────────────────────────────────────────────────────────

    pub async fn tags(&self) -> Result<Vec<Tag>, Error> {
        self.list(&self.at("/repository/tags?order_by=updated"), 2)
            .await
    }

    /// A tag at `reference`; with a message it is annotated.
    pub async fn create_tag(
        &self,
        name: &str,
        reference: &str,
        message: &str,
    ) -> Result<(), Error> {
        let mut body = json!({"tag_name": name.trim(), "ref": reference.trim()});
        if !message.trim().is_empty() {
            body["message"] = json!(message.trim());
        }
        self.act(Method::POST, &self.at("/repository/tags"), Some(body))
            .await
    }

    pub async fn delete_tag(&self, name: &str) -> Result<(), Error> {
        let path = self.at(&format!("/repository/tags/{}", encode(name)));
        self.act(Method::DELETE, &path, None).await
    }
}

#[cfg(test)]
pub mod mock {
    //! A server of a few lines that answers what a test tells it to, and
    //! keeps what it was asked.

    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Clone)]
    pub struct Asked {
        /// `GET /api/v4/user`
        pub line: String,
        pub authorization: String,
        pub body: String,
    }

    pub struct Server {
        pub address: String,
        pub asked: Arc<Mutex<Vec<Asked>>>,
    }

    /// `answers`: for each request in turn, the status, more headers and the
    /// body.
    pub fn serve(answers: Vec<(u16, &'static str, &'static str)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        std::thread::spawn(move || {
            for (status, headers, body) in answers {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let (mut authorization, mut length) = (String::new(), 0);
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    let (name, value) = header.split_once(':').unwrap();
                    match name.to_lowercase().as_str() {
                        "authorization" => authorization = value.trim().to_string(),
                        "content-length" => length = value.trim().parse().unwrap(),
                        _ => {}
                    }
                }
                let mut sent = vec![0; length];
                reader.read_exact(&mut sent).unwrap();
                seen.lock().unwrap().push(Asked {
                    line: line.trim_end().trim_end_matches(" HTTP/1.1").to_string(),
                    authorization,
                    body: String::from_utf8(sent).unwrap(),
                });
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n{body}",
                    body.len()
                );
            }
        });
        Server { address, asked }
    }

    impl Server {
        pub fn lines(&self) -> Vec<String> {
            self.asked
                .lock()
                .unwrap()
                .iter()
                .map(|a| a.line.clone())
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn block<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    fn client(server: &mock::Server) -> Client {
        Client::new(&server.address, "group/sub/app", " tok ").unwrap()
    }

    #[test]
    fn a_token_travels_only_over_https_or_to_this_machine() {
        assert!(Client::new("https://gitlab.com", "a/b", "t").is_ok());
        assert!(Client::new("http://127.0.0.1:80", "a/b", "t").is_ok());
        for bad in [
            "http://gitlab.com",
            "http://localhost.evil.com",
            "http://127.0.0.1.evil.com",
            "ftp://x",
        ] {
            assert!(Client::new(bad, "a/b", "t").is_err(), "{bad}");
        }
    }

    #[test]
    fn a_redirect_is_not_followed_with_the_token() {
        let other = mock::serve(vec![(200, "", "{}")]);
        let location: &'static str =
            Box::leak(format!("Location: {}/api/v4/user\r\n", other.address).into_boxed_str());
        let server = mock::serve(vec![(302, location, "")]);
        assert!(block(client(&server).me()).is_err());
        assert!(other.lines().is_empty());
    }

    #[test]
    fn merge_requests_are_listed_over_the_pages() {
        let server = mock::serve(vec![
            (
                200,
                "x-next-page: 2\r\n",
                r#"[{"iid": 1, "title": "one", "state": "opened"}]"#,
            ),
            (
                200,
                "x-next-page: \r\n",
                r#"[{"iid": 2, "title": "two", "head_pipeline": null}]"#,
            ),
        ]);
        let found = block(client(&server).merge_requests("opened")).unwrap();
        assert_eq!(found.iter().map(|m| m.iid).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(
            server.lines(),
            [
                "GET /api/v4/projects/group%2Fsub%2Fapp/merge_requests?state=opened&order_by=updated_at&per_page=100&page=1",
                "GET /api/v4/projects/group%2Fsub%2Fapp/merge_requests?state=opened&order_by=updated_at&per_page=100&page=2",
            ]
        );
        assert_eq!(server.asked.lock().unwrap()[0].authorization, "Bearer tok");
    }

    #[test]
    fn a_merge_request_is_made_merged_and_deleted() {
        let server = mock::serve(vec![
            (201, "", r#"{"iid": 7, "title": "Draft: t"}"#),
            (200, "", "{}"),
            (204, "", ""),
        ]);
        let api = client(&server);
        let new = NewMergeRequest {
            source: "topic".into(),
            target: "main".into(),
            title: " t ".into(),
            draft: true,
            remove_source: true,
            ..Default::default()
        };
        assert_eq!(block(api.create(&new)).unwrap().iid, 7);
        let how = MergeOptions {
            squash: true,
            ..Default::default()
        };
        block(api.merge(7, "abc", &how)).unwrap();
        block(api.delete(7)).unwrap();
        let asked = server.asked.lock().unwrap().clone();
        let made: Value = serde_json::from_str(&asked[0].body).unwrap();
        assert_eq!(made["title"], "Draft: t");
        assert_eq!(made["source_branch"], "topic");
        assert_eq!(made["remove_source_branch"], true);
        assert!(asked[1].line.ends_with("/merge_requests/7/merge"));
        let merged: Value = serde_json::from_str(&asked[1].body).unwrap();
        assert_eq!(merged["sha"], "abc", "only what was seen is merged");
        assert_eq!(merged["squash"], true);
        assert!(merged.get("merge_when_pipeline_succeeds").is_none());
        assert!(asked[2].line.starts_with("DELETE "));
    }

    #[test]
    fn what_gitlab_refuses_is_said_in_its_words() {
        let server = mock::serve(vec![
            (401, "", r#"{"message": "401 Unauthorized"}"#),
            (
                409,
                "",
                r#"{"message": ["Another open merge request already exists"]}"#,
            ),
            (400, "", r#"{"message": {"base": ["no pipeline"]}}"#),
        ]);
        let api = client(&server);
        assert_eq!(block(api.me()), Err(Error::Unauthorized));
        assert_eq!(
            block(api.create(&NewMergeRequest::default())),
            Err(Error::Server(
                409,
                "Another open merge request already exists".into()
            ))
        );
        assert_eq!(
            block(api.run_pipeline("main")),
            Err(Error::Server(400, "base: no pipeline".into()))
        );
    }

    #[test]
    fn pipelines_jobs_comments_and_tags_ask_for_the_right_things() {
        let server = mock::serve(vec![
            (200, "", "{}"),
            (200, "", "{}"),
            (200, "", "$ cargo test\nok"),
            (201, "", "{}"),
            (201, "", "{}"),
            (200, "", "{}"),
            (201, "", "{}"),
            (204, "", ""),
        ]);
        let api = client(&server);
        block(api.pipeline_action(5, "retry")).unwrap();
        block(api.job_action(9, "play")).unwrap();
        assert_eq!(block(api.job_log(9)).unwrap(), "$ cargo test\nok");
        block(api.comment(3, "hello")).unwrap();
        block(api.reply(3, "d1", "yes")).unwrap();
        block(api.resolve(3, "d1", true)).unwrap();
        block(api.create_tag("v1.0", "main", "")).unwrap();
        block(api.delete_tag("release/1")).unwrap();
        let p = "/api/v4/projects/group%2Fsub%2Fapp";
        assert_eq!(
            server.lines(),
            [
                format!("POST {p}/pipelines/5/retry"),
                format!("POST {p}/jobs/9/play"),
                format!("GET {p}/jobs/9/trace"),
                format!("POST {p}/merge_requests/3/discussions"),
                format!("POST {p}/merge_requests/3/discussions/d1/notes"),
                format!("PUT {p}/merge_requests/3/discussions/d1?resolved=true"),
                format!("POST {p}/repository/tags"),
                format!("DELETE {p}/repository/tags/release%2F1"),
            ]
        );
    }

    #[test]
    fn the_files_of_a_merge_request_are_written_as_patches() {
        let file = FileDiff {
            old_path: "a.rs".into(),
            new_path: "a.rs".into(),
            diff: "@@ -1 +1 @@\n-x\n+y\n".into(),
            ..Default::default()
        };
        assert_eq!(
            file.patch(),
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-x\n+y\n"
        );
        let new = FileDiff {
            new_file: true,
            ..file.clone()
        };
        assert!(new.patch().contains("--- /dev/null\n+++ b/a.rs"));
        let moved = FileDiff {
            old_path: "old".into(),
            new_path: "new".into(),
            renamed_file: true,
            ..Default::default()
        };
        assert_eq!(
            moved.patch(),
            "diff --git a/old b/new\nrename from old\nrename to new\n"
        );
    }

    #[test]
    fn a_thread_is_resolved_when_all_that_can_be_is() {
        let note = |resolvable, resolved| Note {
            resolvable,
            resolved,
            ..Default::default()
        };
        let thread = |notes| Discussion {
            id: "d".into(),
            notes,
        };
        assert!(!thread(vec![note(false, false)]).resolved());
        assert!(!thread(vec![note(true, false)]).resolved());
        assert!(thread(vec![note(true, true), note(false, false)]).resolved());
    }
}
