use futures_util::{future, pin_mut};
use sqlx::postgres::PgPoolOptions;

mod get_trades;
mod resample_trades;
use get_trades::get_trades;
use resample_trades::resample_trades;

// Securities to subscribe to
const SECURITIES: [&str; 2] = ["BINANCE:BTCUSDT", "AAPL"];

// Resample resolution in milliseconds
const RESAMPLE_RESOLUTION: i64 = 1000;

pub async fn run(
    connect_addr: String,
    database_url: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let db_pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url.as_str())
        .await?;

    let resample_trades = resample_trades(&db_pool);
    let get_trades = get_trades(connect_addr, &db_pool);

    pin_mut!(resample_trades, get_trades);
    future::select(resample_trades, get_trades).await;
    Ok(())
}
