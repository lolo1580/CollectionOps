use std::error::Error;

use tokio::net::TcpListener;
use tracing::{info, warn};

use collectionops_backend::{AppConfig, Database, PasswordService};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = AppConfig::from_env()?;

    // Persistence is optional at this stage, but a configured database is a hard startup
    // requirement: starting without it would advertise readiness the service cannot honour.
    if let Some(database_url) = config.database_url() {
        let database = Database::connect_and_migrate(database_url).await?;
        info!("MariaDB connection established and migrations applied");

        if let Some(admin) = config.bootstrap_admin() {
            let created = database
                .provision_bootstrap_admin(admin, &PasswordService::default())
                .await?;

            if created {
                info!("bootstrap administrator created");
            } else {
                info!("bootstrap administrator already present, nothing to do");
            }
        } else {
            warn!("no bootstrap administrator configured");
        }
    } else {
        warn!("COLLECTIONOPS_DATABASE_URL is not set: starting without persistence");
    }

    let address = config.bind_address();
    let listener = TcpListener::bind(address).await?;

    info!(%address, "CollectionOps backend listening");
    axum::serve(listener, collectionops_backend::app())
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for shutdown signal");
    }
}
