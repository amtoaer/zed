use fsevent::EventStream;
use std::{env::args, path::Path, time::Duration};

fn main() {
    let paths = args().skip(1).collect::<Vec<_>>();
    let paths = paths.iter().map(Path::new).collect::<Vec<_>>();
    assert!(paths.len() > 0, "Must pass 1 or more paths as arguments");
    let stream = EventStream::new(&paths, Duration::from_millis(100), |events| {
        eprintln!("event batch");
        for event in events {
            eprintln!("  {:?}", event);
        }
        true
    });
    stream.run();
}
