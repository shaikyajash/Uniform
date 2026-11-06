//! Order Desk: the API and database. Built in B1.

mod db;
mod error;

#[tokio::main]
async fn main() {
    let database_url = std::env::var("DATABASE_URL").expect("set DATABASE_URL, e.g. postgres://localhost/uniform");
    if let Err(error) = db::connect(&database_url).await {
        eprintln!("order-desk: {error}");
        std::process::exit(1);
    }
    println!("order-desk: database ready");
}
