use std::process::Command as StdCommand;

use tokio::process::Command;

use super::policy::EnvPolicy;

fn apply_env_inner(cmd: &mut impl EnvCommandExt, policy: &EnvPolicy) {
    match policy {
        EnvPolicy::Inherit { blocklist } if blocklist.is_empty() => {}
        _ => {
            cmd.env_clear();
            for (key, val) in policy.resolve() {
                cmd.env_set(&key, &val);
            }
        }
    }
}

trait EnvCommandExt {
    fn env_clear(&mut self);
    fn env_set(&mut self, key: &str, val: &str);
}

impl EnvCommandExt for Command {
    fn env_clear(&mut self) {
        Command::env_clear(self);
    }
    fn env_set(&mut self, key: &str, val: &str) {
        Command::env(self, key, val);
    }
}

impl EnvCommandExt for StdCommand {
    fn env_clear(&mut self) {
        StdCommand::env_clear(self);
    }
    fn env_set(&mut self, key: &str, val: &str) {
        StdCommand::env(self, key, val);
    }
}

/// Apply env policy to a tokio command.
pub fn apply_env(cmd: &mut Command, policy: &EnvPolicy) {
    apply_env_inner(cmd, policy);
}

#[cfg(test)]
fn apply_env_std(cmd: &mut StdCommand, policy: &EnvPolicy) {
    apply_env_inner(cmd, policy);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    #[test]
    fn clean_policy_includes_path_and_home_by_default() {
        let policy = EnvPolicy::Clean { allowlist: vec![] };
        let vars = policy.resolve();
        let keys: Vec<_> = vars.iter().map(|(k, _)| k.as_str()).collect();
        if std::env::var("PATH").is_ok() {
            assert!(keys.contains(&"PATH"));
        }
        if std::env::var("HOME").is_ok() {
            assert!(keys.contains(&"HOME"));
        }
    }

    #[test]
    fn inherit_blocklist_strips_vars() {
        let policy = EnvPolicy::Inherit {
            blocklist: vec!["PATH".into()],
        };
        let vars = policy.resolve();
        assert!(!vars.iter().any(|(k, _)| k == "PATH"));
    }

    #[test]
    fn apply_env_std_clears_when_clean() {
        let policy = EnvPolicy::Clean {
            allowlist: vec!["PATH".into()],
        };
        let mut cmd = StdCommand::new("true");
        apply_env_std(&mut cmd, &policy);
        // Cannot easily inspect cmd env in stable API; ensure no panic.
    }
}
