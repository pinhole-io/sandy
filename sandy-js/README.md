# sandy-js

Native Node.js and Bun bindings for Sandy's shell sandbox.

The package delegates backend selection and command execution to Sandy's Rust
`CommandSandbox` implementation. It does not duplicate sandbox policy in
JavaScript.

## Development

```bash
npm ci
npm test
```

The binding requires an available Sandy backend. `auto` selects a supported
backend and fails when no sandbox is available. The unsandboxed backend is not
exposed.

`runShell` accepts an absolute workspace path. Network access is disabled by
default. The command timeout is limited to 600 seconds, and output is bounded
by `maxOutputBytes`.

Node.js is the primary supported runtime. Bun support requires a passing test
on the target Bun version.
