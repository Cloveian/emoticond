//! Search a data file from Rust.
//!
//!     cargo run -p emoticond --example search -- path/to/core.kmj "shy proud"

use emoticond::{Database, OpenOptions, Safety, SearchOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(file), Some(query)) = (args.next(), args.next()) else {
        eprintln!("usage: search DATA.kmj QUERY");
        std::process::exit(2);
    };

    let db = Database::open(OpenOptions::file(file))?;

    let mut opts = SearchOptions::default(); // strict safety, 40 results
    opts.limit = 10;
    opts.safety = Safety::Moderate;

    let result = db.search(&query, &opts);
    println!("read as: {}", db.read(&query, &opts).line);
    for hit in &result.hits {
        println!("{:>2}  {}  ({})", hit.rank, hit.text, hit.id);
    }
    Ok(())
}
