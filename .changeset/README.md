# Changesets

Every pull request adds a changeset here: a file that says how its change moves the version and what
`CHANGELOG.md` should say about it. Merging the pull request releases it (see
[Releases](../docs/development.md#releases)). Because each pull request adds its own file, pull requests
open at the same time don't conflict over the version or the changelog.

`mise run changeset` asks for the kind of change and the summary and writes the file. You can also write
it by hand under any name, like `.changeset/pinned-streams.md`:

```md
---
"briardr": minor
---

**Streams can be pinned.** Pin a stream to keep it at the top of Workspaces, on every device.
```

- `patch` fixes something; `minor` adds something, or changes setup or behavior, which before 1.0 is
  also a minor change; `major` is for 1.0 and later.
- Write the summary for someone upgrading, like the entries in `CHANGELOG.md`: a bold first sentence that
  says what changed, then the detail, wrapped at about 105 columns. It becomes one bullet in the
  version's section, with a link to the pull request.
- A pull request that changes nothing a user notices (docs, CI, tests) adds an empty changeset instead:
  `mise run changeset --empty`. It releases nothing.
