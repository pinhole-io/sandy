//! PyO3 extension `sandy._native` — imported by the `sandy` Python package.

#[cfg(feature = "docker")]
use std::path::Path;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

#[cfg(feature = "docker")]
use crate::service::docker::{DockerConfig, DockerService, SandboxConfig};
#[cfg(feature = "firecracker")]
use crate::service::firecracker::{FirecrackerConfig, FirecrackerService};

/// Pick a Dockerfile path that exists in the current working directory (same as CLI `docker-exec`).
#[cfg(feature = "docker")]
fn default_dockerfile_path() -> String {
    const CANDIDATES: &[&str] = &[
        ".docker/Dockerfile",
        "sandy/.docker/Dockerfile",
        "src/sandy/.docker/Dockerfile",
    ];
    for p in CANDIDATES {
        if Path::new(p).is_file() {
            return (*p).to_string();
        }
    }
    ".docker/Dockerfile".to_string()
}

#[cfg_attr(not(feature = "docker"), allow(unused_variables))]
pub fn init_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    #[cfg(feature = "docker")]
    {
        m.add_function(wrap_pyfunction!(execute_python_docker, m)?)?;
        m.add_function(wrap_pyfunction!(execute_javascript_docker, m)?)?;
    }
    #[cfg(feature = "firecracker")]
    {
        m.add_function(wrap_pyfunction!(execute_python_firecracker, m)?)?;
        m.add_function(wrap_pyfunction!(execute_javascript_firecracker, m)?)?;
    }
    Ok(())
}

#[cfg(feature = "firecracker")]
fn firecracker_config(
    kernel_path: Option<&str>,
    rootfs_path: Option<&str>,
    allow_network: bool,
) -> PyResult<FirecrackerConfig> {
    let mut config = FirecrackerConfig::default();
    if let Some(path) = kernel_path {
        config.kernel_path = path.into();
    }
    if let Some(path) = rootfs_path {
        config.rootfs_path = path.into();
    }
    if allow_network {
        config.tap_device = std::env::var("SANDY_FC_TAP").ok();
        if config.tap_device.is_none() {
            return Err(PyRuntimeError::new_err(
                "network access requires SANDY_FC_TAP",
            ));
        }
    }
    Ok(config)
}

#[cfg(feature = "firecracker")]
#[pyfunction]
#[pyo3(signature = (code, *, kernel_path = None, rootfs_path = None, allow_network = false))]
fn execute_python_firecracker(
    code: &str,
    kernel_path: Option<&str>,
    rootfs_path: Option<&str>,
    allow_network: bool,
) -> PyResult<(String, String, i32)> {
    if code.trim().is_empty() {
        return Err(PyRuntimeError::new_err("code must not be empty"));
    }
    let config = firecracker_config(kernel_path, rootfs_path, allow_network)?;
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| PyRuntimeError::new_err(format!("failed to create runtime: {error}")))?;
    runtime.block_on(async move {
        let service = FirecrackerService::new(config)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        service
            .execute_python_code(code)
            .await
            .map(|result| (result.stdout, result.stderr, result.exit_code))
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    })
}

#[cfg(feature = "firecracker")]
#[pyfunction]
#[pyo3(signature = (code, *, kernel_path = None, rootfs_path = None, allow_network = false))]
fn execute_javascript_firecracker(
    code: &str,
    kernel_path: Option<&str>,
    rootfs_path: Option<&str>,
    allow_network: bool,
) -> PyResult<(String, String, i32)> {
    if code.trim().is_empty() {
        return Err(PyRuntimeError::new_err("code must not be empty"));
    }
    let config = firecracker_config(kernel_path, rootfs_path, allow_network)?;
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| PyRuntimeError::new_err(format!("failed to create runtime: {error}")))?;
    runtime.block_on(async move {
        let service = FirecrackerService::new(config)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
        service
            .execute_javascript_code(code)
            .await
            .map(|result| (result.stdout, result.stderr, result.exit_code))
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    })
}

/// Run Python code in a Docker container (same execution path as `sandy docker-exec`).
#[cfg(feature = "docker")]
#[pyfunction]
#[pyo3(signature = (
    code,
    *,
    allow_network = true,
    no_auto_build = true,
    python_version = "3.13",
    image_name = "sandy",
    image_tag = "latest",
))]
#[allow(clippy::too_many_arguments)]
fn execute_python_docker(
    code: &str,
    allow_network: bool,
    no_auto_build: bool,
    python_version: &str,
    image_name: &str,
    image_tag: &str,
) -> PyResult<(String, String, i32)> {
    if code.trim().is_empty() {
        return Err(PyRuntimeError::new_err("code must not be empty"));
    }

    let sandbox = SandboxConfig {
        network_disabled: !allow_network,
        python_version: python_version.to_string(),
        timeout_secs: 300,
        ..Default::default()
    };

    let config = DockerConfig {
        image_name: image_name.to_string(),
        image_tag: image_tag.to_string(),
        dockerfile_path: default_dockerfile_path(),
        build_context: ".".to_string(),
        sandbox,
    };

    let full_image = format!("{}:{}", image_name, image_tag);

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| PyRuntimeError::new_err(format!("failed to create tokio runtime: {e}")))?;

    rt.block_on(async move {
        let docker = DockerService::new(config)
            .await
            .map_err(|e| PyRuntimeError::new_err(format!("Docker connection failed: {e}")))?;

        match docker.image_exists().await {
            Ok(true) => {}
            Ok(false) => {
                if no_auto_build {
                    return Err(PyRuntimeError::new_err(format!(
                        "Docker image '{full_image}' not found. Build it first, e.g. from workspace root:\n\
                         docker build -f sandy/.docker/Dockerfile -t {image_name}:{image_tag} .\n\
                         See sandy/docs/docker.md."
                    )));
                }
                docker
                    .build_image_via_cli(python_version)
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("docker build failed: {e}")))?;
                if !docker
                    .image_exists()
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("{e}")))?
                {
                    return Err(PyRuntimeError::new_err(format!(
                        "Build finished but image '{full_image}' is still not visible to Docker."
                    )));
                }
            }
            Err(e) => {
                return Err(PyRuntimeError::new_err(format!(
                    "Could not verify Docker image: {e}"
                )));
            }
        }

        docker
            .execute_python_code(code)
            .await
            .map_err(|e| PyRuntimeError::new_err(format!("Docker execution failed: {e}")))
    })
}

/// Run JavaScript code in a Docker container (same execution path as `sandy docker-exec-js`).
#[cfg(feature = "docker")]
#[pyfunction]
#[pyo3(signature = (
    code,
    *,
    allow_network = true,
    no_auto_build = true,
    python_version = "3.13",
    image_name = "sandy",
    image_tag = "latest",
))]
#[allow(clippy::too_many_arguments)]
fn execute_javascript_docker(
    code: &str,
    allow_network: bool,
    no_auto_build: bool,
    python_version: &str,
    image_name: &str,
    image_tag: &str,
) -> PyResult<(String, String, i32)> {
    if code.trim().is_empty() {
        return Err(PyRuntimeError::new_err("code must not be empty"));
    }

    let sandbox = SandboxConfig {
        network_disabled: !allow_network,
        python_version: python_version.to_string(),
        timeout_secs: 300,
        ..Default::default()
    };

    let config = DockerConfig {
        image_name: image_name.to_string(),
        image_tag: image_tag.to_string(),
        dockerfile_path: default_dockerfile_path(),
        build_context: ".".to_string(),
        sandbox,
    };

    let full_image = format!("{}:{}", image_name, image_tag);

    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| PyRuntimeError::new_err(format!("failed to create tokio runtime: {e}")))?;

    rt.block_on(async move {
        let docker = DockerService::new(config)
            .await
            .map_err(|e| PyRuntimeError::new_err(format!("Docker connection failed: {e}")))?;

        match docker.image_exists().await {
            Ok(true) => {}
            Ok(false) => {
                if no_auto_build {
                    return Err(PyRuntimeError::new_err(format!(
                        "Docker image '{full_image}' not found. Build it first, e.g. from workspace root:\n\
                         docker build -f sandy/.docker/Dockerfile -t {image_name}:{image_tag} .\n\
                         See sandy/docs/docker.md."
                    )));
                }
                docker
                    .build_image_via_cli(python_version)
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("docker build failed: {e}")))?;
                if !docker
                    .image_exists()
                    .await
                    .map_err(|e| PyRuntimeError::new_err(format!("{e}")))?
                {
                    return Err(PyRuntimeError::new_err(format!(
                        "Build finished but image '{full_image}' is still not visible to Docker."
                    )));
                }
            }
            Err(e) => {
                return Err(PyRuntimeError::new_err(format!(
                    "Could not verify Docker image: {e}"
                )));
            }
        }

        docker
            .execute_javascript_code(code)
            .await
            .map_err(|e| PyRuntimeError::new_err(format!("Docker execution failed: {e}")))
    })
}
