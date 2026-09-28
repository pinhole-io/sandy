#[cfg(test)]
mod config_tests {
    use crate::service::docker::{DockerConfig, DockerError, SandboxConfig};

    #[test]
    fn test_sandbox_config_defaults() {
        let s = SandboxConfig::default();
        assert_eq!(s.memory_bytes, 512 * 1024 * 1024);
        assert_eq!(s.nano_cpus, 1_000_000_000);
        assert_eq!(s.pids_limit, 64);
        assert!(s.network_disabled);
        assert!(s.readonly_rootfs);
        assert_eq!(s.timeout_secs, 30);
        assert_eq!(s.max_output_bytes, 1024 * 1024);
        assert_eq!(s.max_concurrent, 10);
        assert_eq!(s.container_user, "sandbox");
        assert_eq!(s.python_version, "3.13");
        assert!(s.validate().is_ok());
    }

    #[test]
    fn test_docker_config_defaults() {
        let config = DockerConfig::default();
        assert_eq!(config.image_name, "sandy");
        assert_eq!(config.image_tag, "latest");
        assert_eq!(config.dockerfile_path, "src/sandy/.docker/Dockerfile");
        assert_eq!(config.build_context, ".");
        assert_eq!(config.sandbox.container_user, "sandbox");
    }

    #[test]
    fn test_docker_config_validation_valid() {
        let config = DockerConfig {
            image_name: "my-image".to_string(),
            image_tag: "v1.0.0".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        let config = DockerConfig {
            image_name: "my_image".to_string(),
            image_tag: "latest".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());

        let config = DockerConfig {
            image_name: "my/image".to_string(),
            image_tag: "1.0".to_string(),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_docker_config_validation_invalid_name() {
        let config = DockerConfig {
            image_name: "invalid name".to_string(),
            ..Default::default()
        };
        assert!(matches!(
            config.validate(),
            Err(DockerError::InvalidConfig(_))
        ));

        let config = DockerConfig {
            image_name: "!invalid".to_string(),
            ..Default::default()
        };
        assert!(matches!(
            config.validate(),
            Err(DockerError::InvalidConfig(_))
        ));
    }

    #[test]
    fn test_docker_config_validation_invalid_tag() {
        let config = DockerConfig {
            image_tag: "invalid tag".to_string(),
            ..Default::default()
        };
        assert!(matches!(
            config.validate(),
            Err(DockerError::InvalidConfig(_))
        ));

        let config = DockerConfig {
            image_tag: "/invalid".to_string(),
            ..Default::default()
        };
        assert!(matches!(
            config.validate(),
            Err(DockerError::InvalidConfig(_))
        ));
    }
}
