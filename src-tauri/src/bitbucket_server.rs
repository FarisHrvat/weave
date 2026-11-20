//! Bitbucket Data Center (self-hosted Bitbucket Server) through its REST 1.0
//! API with an HTTP access token. Origins look like
//! https://host[/context]/scm/PROJECT/repo.git or ssh://git@host:7999/project/repo.git.

use crate::{
    external_forge::{
        clean_clone_url, client, field, send, valid_segment, Credential, ExternalComment,
        ExternalInfo, ExternalItem, ExternalPage, ExternalRepository,
    },
    git,
};
use keyring::{Entry, Error as KeyringError};
use reqwest::{blocking::RequestBuilder, Method};
use serde_json::{json, Value};
use url::Url;
use zeroize::Zeroizing;

const LABEL: &str = "Bitbucket Data Center";

#[derive(Debug, PartialEq)]
pub(crate) struct Server {
    /// https://host[/context]
    web: String,
    host: String,
    project: String,
    repository: String,
}

impl Server {
    pub(crate) fn parse(origin: &str) -> Option<Self> {
        let url = Url::parse(origin).ok()?;
        if url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
            return None;
        }
        let host = url.host_str()?.to_string();
        if host == "bitbucket.org" {
            return None;
        }
        let segments: Vec<&str> = url
            .path_segments()?
            .filter(|part| !part.is_empty())
            .collect();
        let (context, project, repository) = match url.scheme() {
            "https" if url.username().is_empty() => {
                let scm = segments.iter().position(|part| *part == "scm")?;
                let [project, repository] = segments.get(scm + 1..)? else {
                    return None;
                };
                (segments[..scm].join("/"), *project, *repository)
            }
            // The SSH port is what identifies a Data Center server (7999 by default).
            "ssh" if url.port().is_some() => match segments.as_slice() {
                [project, repository] => (String::new(), *project, *repository),
                _ => return None,
            },
            _ => return None,
        };
        let repository = repository.trim_end_matches(".git");
        if !valid_segment(project)
            || !valid_segment(repository)
            || !context
                .split('/')
                .all(|part| part.is_empty() || valid_segment(part))
        {
            return None;
        }
        let web = if context.is_empty() {
            format!("https://{host}")
        } else {
            format!("https://{host}/{context}")
        };
        Some(Self {
            web,
            host,
            project: project.to_uppercase(),
            repository: repository.to_lowercase(),
        })
    }

    pub(crate) fn from_repo(path: &str) -> Option<Self> {
        let origin = git::run(path, &["config", "--get", "remote.origin.url"]).ok()?;
        Self::parse(origin.trim())
    }

    fn repo_api(&self) -> String {
        format!(
            "{}/rest/api/1.0/projects/{}/repos/{}",
            self.web, self.project, self.repository
        )
    }

    fn project_api(&self) -> String {
        format!("{}/rest/api/1.0/projects/{}/repos", self.web, self.project)
    }

    fn entry(&self) -> Result<Entry, String> {
        Entry::new(
            "app.weave.desktop",
            &format!("external:{LABEL}:{}", self.host),
        )
        .map_err(|error| format!("Could not open the system keychain: {error}"))
    }

    fn credential(&self) -> Result<Credential, String> {
        let saved = self.entry()?.get_password().map_err(|error| match error {
            KeyringError::NoEntry => format!("Connect {LABEL} before using its online tools."),
            _ => format!("Could not read the system keychain: {error}"),
        })?;
        serde_json::from_str(&saved)
            .map_err(|_| "The saved account is invalid. Disconnect and connect it again.".into())
    }

    pub(crate) fn info(&self) -> Result<ExternalInfo, String> {
        let connected = match self.entry()?.get_password() {
            Ok(_) => true,
            Err(KeyringError::NoEntry) => false,
            Err(error) => return Err(format!("Could not read the system keychain: {error}")),
        };
        Ok(ExternalInfo {
            provider: LABEL.into(),
            organization: self.project.clone(),
            repository: self.repository.clone(),
            connected,
            url: format!(
                "{}/projects/{}/repos/{}",
                self.web, self.project, self.repository
            ),
        })
    }

    pub(crate) fn connect(&self, username: &str, token: &str) -> Result<(), String> {
        let credential = Credential {
            email: username.trim().into(),
            token: Zeroizing::new(token.trim().into()),
        };
        if credential.token.is_empty() || credential.email.is_empty() {
            return Err("Enter your Bitbucket username and an HTTP access token.".into());
        }
        self.call(&credential, Method::GET, &self.repo_api(), None)?;
        let saved = serde_json::to_string(&credential).map_err(|error| error.to_string())?;
        self.entry()?
            .set_password(&saved)
            .map_err(|error| format!("Could not save the token in the system keychain: {error}"))
    }

    pub(crate) fn disconnect(&self) -> Result<(), String> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => Err(format!(
                "Could not remove the token from the system keychain: {error}"
            )),
        }
    }

    fn call(
        &self,
        credential: &Credential,
        method: Method,
        url: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let mut request: RequestBuilder = client()?
            .request(method, url)
            .bearer_auth(credential.token.as_str())
            .header("Accept", "application/json")
            .header("X-Atlassian-Token", "no-check");
        if let Some(body) = body {
            request = request.json(&body);
        }
        send(request, LABEL)
    }

    fn link(value: &Value) -> String {
        value["links"]["self"]
            .as_array()
            .and_then(|links| links.first())
            .map(|link| field(link, "href").to_string())
            .unwrap_or_default()
    }

    pub(crate) fn list(&self, kind: &str, state: &str, page: u32) -> Result<ExternalPage, String> {
        self.list_at(&self.credential()?, kind, state, page, &self.repo_api())
    }

    fn list_at(
        &self,
        credential: &Credential,
        kind: &str,
        state: &str,
        page: u32,
        base: &str,
    ) -> Result<ExternalPage, String> {
        if kind == "ci" {
            return self.builds_at(credential, page, base, &self.web);
        }
        if page == 0 || page > 1000 {
            return Err("Choose a page between 1 and 1000.".into());
        }
        let state = match state {
            "open" | "" => "OPEN",
            "merged" => "MERGED",
            "declined" => "DECLINED",
            "all" => "ALL",
            _ => return Err("Choose open, merged, declined or all pull requests.".into()),
        };
        let url = format!(
            "{base}/pull-requests?state={state}&start={}&limit=30",
            (page - 1) * 30
        );
        let result = self.call(credential, Method::GET, &url, None)?;
        let items = result["values"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|value| ExternalItem {
                number: value["id"].as_i64().unwrap_or_default(),
                title: field(value, "title").into(),
                state: field(value, "state").into(),
                url: Self::link(value),
                detail: value
                    .pointer("/author/user/displayName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .into(),
            })
            .collect();
        Ok(ExternalPage {
            items,
            has_more: result["isLastPage"].as_bool() == Some(false),
        })
    }

    /// Build results for the newest commits, from the build-status API.
    fn builds_at(
        &self,
        credential: &Credential,
        page: u32,
        base: &str,
        web: &str,
    ) -> Result<ExternalPage, String> {
        if page == 0 || page > 100 {
            return Err("Choose a page between 1 and 100.".into());
        }
        let commits = self.call(
            credential,
            Method::GET,
            &format!("{base}/commits?start={}&limit=30", (page - 1) * 30),
            None,
        )?;
        let values: Vec<&Value> = commits["values"].as_array().into_iter().flatten().collect();
        let ids: Vec<&str> = values
            .iter()
            .filter_map(|commit| commit["id"].as_str())
            .collect();
        let stats = if ids.is_empty() {
            Value::Null
        } else {
            self.call(
                credential,
                Method::POST,
                &format!("{web}/rest/build-status/1.0/commits/stats"),
                Some(json!(ids)),
            )?
        };
        let items = values
            .iter()
            .filter_map(|commit| {
                let id = commit["id"].as_str()?;
                let counts = &stats[id];
                let count = |key: &str| counts[key].as_i64().unwrap_or(0);
                let state = if count("failed") > 0 {
                    "FAILED"
                } else if count("inProgress") > 0 {
                    "INPROGRESS"
                } else if count("successful") > 0 {
                    "SUCCESSFUL"
                } else {
                    "no builds"
                };
                Some(ExternalItem {
                    number: 0,
                    title: format!(
                        "{} {}",
                        field(commit, "displayId"),
                        field(commit, "message").lines().next().unwrap_or("")
                    ),
                    state: state.into(),
                    url: format!(
                        "{}/projects/{}/repos/{}/commits/{id}",
                        self.web, self.project, self.repository
                    ),
                    detail: format!(
                        "{} passed, {} running, {} failed",
                        count("successful"),
                        count("inProgress"),
                        count("failed")
                    ),
                })
            })
            .collect();
        Ok(ExternalPage {
            items,
            has_more: commits["isLastPage"].as_bool() == Some(false),
        })
    }

    pub(crate) fn create_review(
        &self,
        title: &str,
        description: &str,
        source: &str,
        target: &str,
    ) -> Result<(), String> {
        let body = json!({
            "title": title.trim(),
            "description": description,
            "fromRef": {"id": format!("refs/heads/{source}")},
            "toRef": {"id": format!("refs/heads/{target}")},
        });
        self.call(
            &self.credential()?,
            Method::POST,
            &format!("{}/pull-requests", self.repo_api()),
            Some(body),
        )?;
        Ok(())
    }

    pub(crate) fn merge(&self, number: i64) -> Result<(), String> {
        self.merge_at(&self.credential()?, number, &self.repo_api())
    }

    fn merge_at(&self, credential: &Credential, number: i64, base: &str) -> Result<(), String> {
        let current = self.call(
            credential,
            Method::GET,
            &format!("{base}/pull-requests/{number}"),
            None,
        )?;
        let version = current["version"]
            .as_i64()
            .ok_or("Bitbucket did not return the pull request version. Refresh and try again.")?;
        self.call(
            credential,
            Method::POST,
            &format!("{base}/pull-requests/{number}/merge?version={version}"),
            None,
        )?;
        Ok(())
    }

    pub(crate) fn approve(&self, number: i64) -> Result<(), String> {
        self.call(
            &self.credential()?,
            Method::POST,
            &format!("{}/pull-requests/{number}/approve", self.repo_api()),
            None,
        )?;
        Ok(())
    }

    pub(crate) fn comments(&self, number: i64) -> Result<Vec<ExternalComment>, String> {
        self.comments_at(&self.credential()?, number, &self.repo_api())
    }

    fn comments_at(
        &self,
        credential: &Credential,
        number: i64,
        base: &str,
    ) -> Result<Vec<ExternalComment>, String> {
        let result = self.call(
            credential,
            Method::GET,
            &format!("{base}/pull-requests/{number}/activities?limit=100"),
            None,
        )?;
        let mut comments: Vec<ExternalComment> = result["values"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|activity| activity["action"] == "COMMENTED")
            .map(|activity| {
                let comment = &activity["comment"];
                let location = match (
                    activity
                        .pointer("/commentAnchor/path")
                        .and_then(Value::as_str),
                    activity
                        .pointer("/commentAnchor/line")
                        .and_then(Value::as_i64),
                ) {
                    (Some(path), Some(line)) => format!("{path}:{line}"),
                    (Some(path), None) => path.into(),
                    _ => String::new(),
                };
                let created = comment["createdDate"]
                    .as_i64()
                    .and_then(|ms| chrono::DateTime::from_timestamp(ms / 1000, 0))
                    .map(|time| time.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                ExternalComment {
                    id: comment["id"].as_i64().unwrap_or_default(),
                    thread: 0,
                    author: comment
                        .pointer("/author/displayName")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    body: field(comment, "text").into(),
                    created,
                    location,
                }
            })
            .collect();
        // Activities arrive newest first.
        comments.reverse();
        Ok(comments)
    }

    pub(crate) fn comment(
        &self,
        number: i64,
        body: &str,
        parent: Option<i64>,
    ) -> Result<(), String> {
        let body = body.trim();
        if body.is_empty() || body.len() > 32_000 {
            return Err("Write a comment of up to 32,000 characters.".into());
        }
        let payload = match parent {
            Some(id) => json!({"text": body, "parent": {"id": id}}),
            None => json!({"text": body}),
        };
        self.call(
            &self.credential()?,
            Method::POST,
            &format!("{}/pull-requests/{number}/comments", self.repo_api()),
            Some(payload),
        )?;
        Ok(())
    }

    fn repository(&self, value: &Value) -> ExternalRepository {
        ExternalRepository {
            name: field(value, "name").into(),
            clone_url: value["links"]["clone"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|link| link["name"] == "http")
                .map(|link| clean_clone_url(field(link, "href")))
                .unwrap_or_default(),
            web_url: Self::link(value),
            private: value["public"].as_bool() != Some(true),
        }
    }

    pub(crate) fn repositories(&self) -> Result<Vec<ExternalRepository>, String> {
        self.repositories_at(&self.credential()?, &self.project_api())
    }

    fn repositories_at(
        &self,
        credential: &Credential,
        base: &str,
    ) -> Result<Vec<ExternalRepository>, String> {
        let result = self.call(credential, Method::GET, &format!("{base}?limit=100"), None)?;
        let mut repositories: Vec<_> = result["values"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|value| self.repository(value))
            .collect();
        repositories.sort_by_key(|repo| repo.name.to_lowercase());
        Ok(repositories)
    }

    pub(crate) fn create_repository(
        &self,
        name: &str,
        private: bool,
    ) -> Result<ExternalRepository, String> {
        let name = name.trim();
        if name.is_empty() || name.len() > 128 || !valid_segment(name) {
            return Err(
                "Use letters, numbers, dots, dashes or underscores for the repository name.".into(),
            );
        }
        let created = self.call(
            &self.credential()?,
            Method::POST,
            &self.project_api(),
            Some(json!({"name": name, "scmId": "git", "public": !private})),
        )?;
        Ok(self.repository(&created))
    }

    /// HTTPS Git credentials for this server, from the saved account.
    pub(crate) fn git_auth(url: &str) -> Result<Option<crate::forge::GitAuth>, String> {
        let Some(server) = Self::parse(url).filter(|_| url.starts_with("https://")) else {
            return Ok(None);
        };
        let credential = match server.entry()?.get_password() {
            Ok(saved) => serde_json::from_str::<Credential>(&saved)
                .map_err(|_| "The saved account is invalid. Disconnect and connect it again.")?,
            Err(KeyringError::NoEntry) => return Ok(None),
            Err(error) => return Err(format!("Could not read the system keychain: {error}")),
        };
        Ok(Some(crate::forge::GitAuth {
            username: credential.email,
            host: server.host,
            token: credential.token,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    fn serve(replies: Vec<&'static str>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut seen = Vec::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut bytes = [0_u8; 4096];
                loop {
                    let count = stream.read(&mut bytes).unwrap();
                    request.extend_from_slice(&bytes[..count]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{reply}", reply.len()).unwrap();
                seen.push(String::from_utf8(request).unwrap());
            }
            seen
        });
        (base, handle)
    }

    fn token() -> Credential {
        Credential {
            email: "ada".into(),
            token: Zeroizing::new("tok".into()),
        }
    }

    #[test]
    fn recognises_data_center_origins() {
        let server = Server::parse("https://git.example.test/bitbucket/scm/OPS/tools.git").unwrap();
        assert_eq!(
            server.repo_api(),
            "https://git.example.test/bitbucket/rest/api/1.0/projects/OPS/repos/tools"
        );
        let ssh = Server::parse("ssh://git@git.example.test:7999/ops/tools.git").unwrap();
        assert_eq!(ssh.project, "OPS");
        assert_eq!(ssh.web, "https://git.example.test");
        assert!(Server::parse("https://bitbucket.org/team/repo").is_none());
        assert!(Server::parse("https://github.com/a/b.git").is_none());
        assert!(Server::parse("https://user@git.example.test/scm/OPS/tools.git").is_none());
    }

    #[test]
    fn lists_merges_and_reads_comments_with_a_bearer_token() {
        let server = Server::parse("https://git.example.test/scm/OPS/tools.git").unwrap();
        let (base, handle) = serve(vec![
            r#"{"values":[{"id":3,"title":"Add CI","state":"OPEN","author":{"user":{"displayName":"Ada"}},"links":{"self":[{"href":"https://git.example.test/projects/OPS/repos/tools/pull-requests/3"}]}}],"isLastPage":false}"#,
            r#"{"version":4}"#,
            "{}",
            r#"{"values":[{"action":"COMMENTED","comment":{"text":"Second","author":{"displayName":"Lin"},"createdDate":1767225600000}},{"action":"APPROVED"},{"action":"COMMENTED","comment":{"text":"First","author":{"displayName":"Ada"},"createdDate":1767139200000},"commentAnchor":{"path":"ci.yml","line":2}}]}"#,
        ]);
        let page = server
            .list_at(&token(), "reviews", "merged", 2, &base)
            .unwrap();
        assert_eq!(page.items[0].detail, "Ada");
        assert!(page.has_more);
        server.merge_at(&token(), 3, &base).unwrap();
        let comments = server.comments_at(&token(), 3, &base).unwrap();
        assert_eq!(comments[0].body, "First");
        assert_eq!(comments[0].location, "ci.yml:2");
        assert_eq!(comments[1].created, "2026-01-01");
        let requests = handle.join().unwrap();
        assert!(
            requests[0].starts_with("GET /pull-requests?state=MERGED&start=30&limit=30 HTTP/1.1")
        );
        assert!(requests[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer tok"));
        assert!(requests[2].starts_with("POST /pull-requests/3/merge?version=4 HTTP/1.1"));
        assert!(server.list_at(&token(), "ci", "open", 0, &base).is_err());
    }

    #[test]
    fn lists_project_repositories() {
        let server = Server::parse("https://git.example.test/scm/OPS/tools.git").unwrap();
        let (base, handle) = serve(vec![
            r#"{"values":[{"name":"tools","public":false,"links":{"clone":[{"name":"ssh","href":"ssh://git@git.example.test:7999/ops/tools.git"},{"name":"http","href":"https://ada@git.example.test/scm/ops/tools.git"}],"self":[{"href":"https://git.example.test/projects/OPS/repos/tools/browse"}]}}]}"#,
        ]);
        let repos = server.repositories_at(&token(), &base).unwrap();
        handle.join().unwrap();
        assert_eq!(
            repos[0].clone_url,
            "https://git.example.test/scm/ops/tools.git"
        );
        assert!(repos[0].private);
    }

    #[test]
    fn lists_build_results_per_commit() {
        let server = Server::parse("https://git.example.test/scm/OPS/tools.git").unwrap();
        let (base, handle) = serve(vec![
            r#"{"values":[{"id":"aaa111","displayId":"aaa1","message":"Fix login\nmore"},{"id":"bbb222","displayId":"bbb2","message":"Docs"}],"isLastPage":true}"#,
            r#"{"aaa111":{"successful":2,"inProgress":0,"failed":1},"bbb222":{"successful":1,"inProgress":0,"failed":0}}"#,
        ]);
        let page = server.builds_at(&token(), 1, &base, &base).unwrap();
        let requests = handle.join().unwrap();
        assert_eq!(page.items[0].state, "FAILED");
        assert_eq!(page.items[0].title, "aaa1 Fix login");
        assert_eq!(page.items[1].state, "SUCCESSFUL");
        assert!(requests[1].starts_with("POST /rest/build-status/1.0/commits/stats HTTP/1.1"));
        assert!(requests[1].ends_with(r#"["aaa111","bbb222"]"#));
    }
}
