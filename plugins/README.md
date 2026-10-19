# plugins/

Briardr's built-in plugins, and the template for new ones.

| Folder | What |
| --- | --- |
| `files/`, `changes/`, `preview/`, `docs/`, `notes/`, `usage/` | The built-in plugins: the Files, Changes, Preview, Docs and Notes tabs in every stream, and Usage |
| `template/` | What `briardr create plugin` copies to start a new plugin |

The built-ins are made exactly like the plugins anyone writes: a `briardr-plugin.json`, a client in
`src/` behind `definePlugin`, and nothing from Briardr but `@briardr/sdk`. No imports from `web/src`, no
private props. When one needs something Briardr doesn't offer plugins yet, it goes into the SDK
(`web/src/sdk/`) for every plugin. `test/builtin-plugins.test.ts` checks this, and also builds each
one as an installed plugin.

They differ only in delivery: the app's build bundles them (so they work on a first launch and
offline), they're on until turned off in Settings → Plugins, and they use Briardr's own API instead of a
plugin backend. See [docs/PLUGINS.md](../docs/PLUGINS.md).
