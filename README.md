# GitLab — an Ardoise Editor plugin

GitLab (gitlab.com or your own server) in [Ardoise Editor](https://github.com/ardoise-editor/ardoise), as a tab of the Git panel. It works with or without the Git plugin, and needs `git` and Ardoise 2.8 or later.

- **Merge requests**: the open, merged or closed ones; make one from the branch checked out; read one with its description, its files and their diffs in a tab, its comments (answer, resolve, comment), its pipeline; approve, merge (squash, delete the source branch, once the pipeline passes), close, reopen, delete.
- **Pipelines**: the latest ones, their jobs and logs; run one on a branch or a tag; retry, cancel, delete; retry, cancel or start a job.
- **Tags**: list, create (annotated with a message), delete.

The project is the one of the `origin` remote. For each server a personal access token with the `api` scope is asked for in the panel. It is kept in the system keychain (or a file only you can read when there is none) and **sent only to the server you typed it for**, over HTTPS.

Merging asks GitLab for the commit you saw: if the branch moved since, GitLab refuses.

Deleting a merge request is allowed by GitLab to owners and administrators only; others close it.

## Build

```sh
cargo test
cargo build --release && mkdir -p bin && cp target/release/gitlab bin/
```

## License

MIT
