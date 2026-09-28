use sandy::{DockerConfig, DockerService};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
#[ignore] // Ignored by default as it requires a Docker daemon
async fn test_docker_service_connection() {
    let config = DockerConfig::default();
    let service_result = DockerService::new(config).await;

    match service_result {
        Ok(service) => {
            // Connection successful
            let exists_result = service.image_exists().await;
            assert!(exists_result.is_ok());
        }
        Err(e) => {
            // Connection failed - this is expected in CI or environments without Docker
            eprintln!("Skipping test due to Docker connection failure: {}", e);
        }
    }
}

#[tokio::test]
#[ignore] // Ignored by default as it requires a Docker daemon
async fn test_docker_execution_flow() {
    // Only run if we can connect to Docker
    let config = DockerConfig::default();
    let service = match DockerService::new(config).await {
        Ok(s) => s,
        Err(_) => return, // Skip if no Docker
    };

    // Check if image exists, otherwise skip
    if !service.image_exists().await.unwrap_or(false) {
        eprintln!("Skipping execution test: sandy image not found");
        return;
    }

    // Simple execution test
    let code = "print('Hello from integration test')";
    let result = timeout(Duration::from_secs(30), service.execute_python_code(code)).await;

    match result {
        Ok(Ok((stdout, stderr, exit_code))) => {
            assert_eq!(exit_code, 0);
            assert!(stdout.contains("Hello from integration test"));
            assert!(stderr.is_empty());
        }
        Ok(Err(e)) => {
            panic!("Execution failed: {}", e);
        }
        Err(_) => {
            panic!("Execution timed out");
        }
    }
}

#[tokio::test]
#[ignore] // Ignored by default as it requires a Docker daemon
async fn test_docker_javascript_execution_flow() {
    let config = DockerConfig::default();
    let service = match DockerService::new(config).await {
        Ok(s) => s,
        Err(_) => return,
    };

    if !service.image_exists().await.unwrap_or(false) {
        eprintln!("Skipping execution test: sandy image not found");
        return;
    }

    let code = "console.log('Hello from JS integration test')";
    let result = timeout(
        Duration::from_secs(30),
        service.execute_javascript_code(code),
    )
    .await;

    match result {
        Ok(Ok((stdout, stderr, exit_code))) => {
            assert_eq!(exit_code, 0);
            assert!(stdout.contains("Hello from JS integration test"));
            assert!(stderr.is_empty());
        }
        Ok(Err(e)) => {
            panic!("JS execution failed: {}", e);
        }
        Err(_) => {
            panic!("JS execution timed out");
        }
    }
}
