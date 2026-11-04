#[path = "tokenizer/support.rs"]
mod support;

#[test]
fn large_file_boundaries_and_worker_fixtures_match_python() {
    support::compare("large");
    for case in support::oracle().cases.iter().filter(|case| {
        matches!(
            case.name.as_str(),
            "large-files-ARCHITECTURE" | "large-files-POOL" | "large-file-boundary-+0"
        )
    }) {
        let text = case.expanded();
        let mut times = Vec::new();
        for _ in 0..5 {
            let start = std::time::Instant::now();
            let actual = support::count(support::tokenizer(), std::hint::black_box(&text));
            times.push(start.elapsed());
            assert_eq!(actual, case.tokens);
        }
        times.sort();
        println!(
            "Rust benchmark: {} bytes={} median_ms={:.3} calls=5 profile={}",
            case.name,
            text.len(),
            times[2].as_secs_f64() * 1000.0,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
    }
}
