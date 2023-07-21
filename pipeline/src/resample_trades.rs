use crate::{RESAMPLE_FREQUENCY, SECURITIES};

use futures::{future::join_all, TryStreamExt};
use sqlx::Row;
use tokio::{
    io::AsyncWriteExt,
    time::{interval, sleep, Duration},
};

pub async fn resample_trades(db_pool: &sqlx::SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    // Maximum update frequency in milliseconds
    let mut interval = interval(Duration::from_millis(10));

    // Past recheck leap in milliseconds
    // This is the amount of time in milliseconds that the resampler will
    // recheck for wrong data in the past
    // this is usefull because sometimes, the websocket connection will
    // give really out of order
    let past_recheck_leap = std::cmp::max(2 * RESAMPLE_FREQUENCY, 2000);

    loop {
        let mut tasks = Vec::new();
        for security in SECURITIES {
            tasks.push(async move {
                let max_timestamp: Result<(i64,), _> =
                    sqlx::query_as(r#"SELECT MAX(timestamp) FROM trades_raw WHERE security = ?;"#)
                        .bind(security)
                        .fetch_one(db_pool)
                        .await;

                let Ok(max_timestamp) = max_timestamp else {return};

                let max_timestamp =
                    max_timestamp.0.div_euclid(RESAMPLE_FREQUENCY) * RESAMPLE_FREQUENCY;

                //println!("max {}", max_timestamp);

                let min_timestamp: (i64,) = sqlx::query_as(
                    r#"SELECT MAX(timestamp) FROM trades_resampled WHERE security = ?;"#,
                )
                .bind(security)
                .fetch_one(db_pool)
                .await
                .unwrap_or_default();

                let min_timestamp = min_timestamp.0 - past_recheck_leap;

                //println!("min {}", min_timestamp);

                let query = format!(
                    r#"SELECT rstimestamp as timestamp, price FROM (
            SELECT rstimestamp, max(timestamp), price FROM (
                SELECT price, (timestamp/{frequency})*{frequency} as rstimestamp, volume, timestamp
                FROM trades_raw
                WHERE timestamp <= ? AND timestamp > ? AND
                security = ?
            )
            GROUP BY rstimestamp ORDER BY timestamp ASC
            ) LIMIT 0,100"#,
                    frequency = RESAMPLE_FREQUENCY
                );
                let mut rows = sqlx::query(query.as_str())
                    .bind(max_timestamp)
                    .bind(min_timestamp)
                    .bind(security)
                    .fetch(db_pool);

                let mut prev_timestamp: Option<i64> = None;
                while let Ok(Some(row)) = rows.try_next().await {
                    let Ok::<f64, _>(price) = row.try_get("price") else { continue };
                    let Ok::<i64, _>(timestamp) = row.try_get("timestamp") else { continue };

                    // Fill time series gaps with the previous value
                    if let Some(mut prev_timestamp) = prev_timestamp {
                        while prev_timestamp < timestamp {
                            prev_timestamp += RESAMPLE_FREQUENCY;

                            while let Err(e) = sqlx::query(
                                "INSERT INTO trades_resampled (price, security, timestamp)
                    VALUES (?, ?, ?)
                    ON CONFLICT (security, timestamp) DO UPDATE SET price = EXCLUDED.price;
                    ;",
                            )
                            .bind(price)
                            .bind(security)
                            .bind(prev_timestamp)
                            .execute(db_pool)
                            .await

                            {
                                tokio::io::stdout()
                                    .write_all(
                                        format!("Error while inserting data into trades_resampled table: {e}")
                                            .as_bytes(),
                                    )
                                    .await
                                    .unwrap_or_default();
                                sleep(Duration::from_millis(50)).await;
                            }
                        }
                    }
                    prev_timestamp = Some(timestamp);

                    while let Err(e) = sqlx::query(
                        "INSERT INTO trades_resampled (price, security, timestamp)
                VALUES (?, ?, ?)
                ON CONFLICT (security, timestamp) DO UPDATE SET price = EXCLUDED.price;
                ;",
                    )
                    .bind(price)
                    .bind(security)
                    .bind(timestamp)
                    .execute(db_pool)
                    .await
                    {
                        tokio::io::stdout()
                            .write_all(
                                format!("Error while inserting data into trades_resampled table: {e}")
                                    .as_bytes(),
                            )
                            .await
                            .unwrap_or_default();
                        sleep(Duration::from_millis(50)).await;
                    }
                }
            });
        }
        join_all(tasks).await;
        interval.tick().await;
    }
}
