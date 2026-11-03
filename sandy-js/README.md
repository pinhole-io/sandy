# sandy-js

Node.js and Bun bindings for the Sandy shell sandbox.

The package calls Sandy's Rust `CommandSandbox`. It does not copy the sandbox policy into JavaScript.

This package is not published to npm. Build it from this repository.

## Requirements

- Node.js 18 or newer
- A Rust toolchain, when you build from this repository
- An available Sandy backend on the host

`auto` selects a supported backend. The call fails when that backend is missing. The unsandboxed backend is rejected.

## Build

From `sandy-js/`:

```bash
npm ci
npm test
```

- `npm run build:native` builds the release addon.
- `npm run build:native:debug` builds the debug addon.
- `npm test` builds the debug addon and runs `test/run.mjs`.

The binding crate depends on `sandy` with `exec-sandbox` only. It does not enable the `docker` or `firecracker` features of the parent crate.

## Run a command

`runShell` requires an absolute path to an existing directory.

Network access is off unless `allowNetwork` is true. The process environment is empty unless `envAllowlist` names keys to copy. The default timeout is 120 seconds. Set `timeoutSecs` from 1 to 600. The default output limit is 1 MiB per stream. `maxOutputBytes` cannot exceed 16 MiB.

```js
const { runShell, backendInfo } = require("sandy-js");

async function main() {
  console.log(backendInfo());
  const result = await runShell({
    command: "printf hello",
    workspace: process.cwd(),
    allowNetwork: false,
  });
  console.log(result.stdout);
  console.log(result.backend, result.exitCode);
}

main();
```

| Export | What it returns |
| --- | --- |
| `runShell(options)` | `backend`, `stdout`, `stderr`, `exitCode`, and truncation flags |
| `backendInfo()` | One `{ id, available }` row per known backend |
| `needsBwrapApparmor()` | `true` when this host needs the AppArmor profile |
| `bwrapUnavailableReason()` | A string, or `null` when bubblewrap can start |
| `bwrapApparmorProfile()` | The profile text for `/usr/bin/bwrap` |
| `resetBwrapProbe()` | Clears the cached bubblewrap probe |

`backend` accepts `auto`, `bubblewrap`, `docker`, or `firecracker`. `docker` and `firecracker` work only when that backend is compiled into the addon. `none` returns an error.

Ubuntu 24.04 and newer can block `bwrap` user namespaces. Install `contrib/apparmor/bwrap` from the repository root. The steps are in the [README](../README.md#bubblewrap-on-ubuntu-2404-or-newer).

## License

The license is [MIT](../LICENSE).
