use tracing_subscriber::filter::{EnvFilter, LevelFilter};

const TRACING_ENV: &str = "BTT_LOG";

#[tracing::instrument]
pub fn setup_logger() {
    let env_filter = EnvFilter::builder()
        .with_env_var(TRACING_ENV)
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();

    let subscriber = tracing_subscriber::fmt().compact().with_env_filter(env_filter).finish();

    tracing::subscriber::set_global_default(subscriber).expect("Error setting a global tracing::subscriber");
}
