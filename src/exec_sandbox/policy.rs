use std::path::PathBuf;

/// How sandboxed commands inherit environment variables from the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvPolicy {
    /// Inherit all host env vars, optionally stripping blocklisted names.
    Inherit { blocklist: Vec<String> },
    /// Clear host env and restore only allowlisted vars (plus minimal defaults).
    Clean { allowlist: Vec<String> },
}

impl Default for EnvPolicy {
    fn default() -> Self {
        Self::Inherit {
            blocklist: Vec::new(),
        }
    }
}

impl EnvPolicy {
    /// Build the env map to pass to a subprocess.
    pub fn resolve(&self) -> Vec<(String, String)> {
        match self {
            EnvPolicy::Inherit { blocklist } => {
                let blocked: std::collections::HashSet<_> =
                    blocklist.iter().map(|s| s.as_str()).collect();
                std::env::vars()
                    .filter(|(k, _)| !blocked.contains(k.as_str()))
                    .collect()
            }
            EnvPolicy::Clean { allowlist } => {
                let mut out = Vec::new();
                let allowed: std::collections::HashSet<_> =
                    allowlist.iter().map(|s| s.as_str()).collect();
                for key in &["PATH", "HOME", "USER", "LANG", "LC_ALL", "TERM"] {
                    if allowed.contains(key) || allowlist.is_empty() {
                        if let Ok(val) = std::env::var(key) {
                            out.push((key.to_string(), val));
                        }
                    }
                }
                for key in allowlist {
                    if out.iter().any(|(k, _)| k == key) {
                        continue;
                    }
                    if let Ok(val) = std::env::var(key) {
                        out.push((key.clone(), val));
                    }
                }
                out
            }
        }
    }
}

/// Paths, cwd, and network policy for a sandboxed shell command.
#[derive(Debug, Clone)]
pub struct ExecPolicy {
    pub cwd: PathBuf,
    /// Host paths mounted or bound read-write inside the sandbox (includes `cwd` when applicable).
    pub writable_roots: Vec<PathBuf>,
    pub allow_network: bool,
    pub env: EnvPolicy,
    pub max_output_bytes: usize,
    /// Applied after [`EnvPolicy::resolve`]. A later key replaces an earlier one.
    pub env_overrides: Vec<(String, String)>,
}

impl ExecPolicy {
    pub fn workspace(cwd: PathBuf) -> Self {
        let extra = super::git_roots::workspace_git_writable_roots(&cwd);
        let mut writable_roots = vec![cwd.clone()];
        for root in extra {
            if !writable_roots.iter().any(|path| path == &root) {
                writable_roots.push(root);
            }
        }
        Self {
            writable_roots,
            cwd,
            allow_network: false,
            env: EnvPolicy::default(),
            max_output_bytes: 1024 * 1024,
            env_overrides: Vec::new(),
        }
    }

    pub fn with_extra_writable_roots(mut self, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        for root in roots {
            if !self.writable_roots.iter().any(|p| p == &root) {
                self.writable_roots.push(root);
            }
        }
        self
    }

    pub fn with_allow_network(mut self, allow_network: bool) -> Self {
        self.allow_network = allow_network;
        self
    }

    pub fn with_env_policy(mut self, env: EnvPolicy) -> Self {
        self.env = env;
        self
    }

    pub fn with_max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes.max(1);
        self
    }
}
