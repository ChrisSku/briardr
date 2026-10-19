# {{name}} (a Briardr plugin)

{{description}}

This folder is a plugin for Briardr, the remote window into Herdr on {{host}}. It's installed (linked
into Briardr) and turned on. Read the `briardr-plugins` skill before changing anything: it has the
manifest, the `@briardr/sdk` API, the backend protocol, the UI conventions and the rules plugins follow.

- `briardr-plugin.json`: ID, name, what the plugin adds (`contributes`) and the backend command.
- `src/index.ts`: the client entry, `definePlugin({...})`; `src/*.svelte`: Svelte 5 components (runes).
{{#backend}}
- `server/main.ts`: the backend (Bun), started by Briardr; `server/briardr.ts` is the protocol, don't edit it.
{{/backend}}

After every change:

```sh
{{briardr_bin}} plugin build .   # devices with Briardr open reload the plugin by themselves
{{briardr_bin}} plugin check .   # type errors in this plugin
```

`{{briardr_bin}} plugin dev .` rebuilds on every save instead (it keeps running). The Briardr checkout is
{{briardr}}. Its built-in plugins (Files, Changes, Notes, …) are made the same way, with nothing but
`@briardr/sdk`: read the closest one in `plugins/<id>/` there for examples, and `web/src/sdk/index.ts`.
