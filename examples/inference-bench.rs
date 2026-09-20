//! Reproducible cold/edit benchmark; timings include input preparation and assembly.
//! cargo run --release --example inference-bench -- <workers> <groups> <samples> [shape]
use ruddy::{
    execution::Execution,
    inference::{Session, Trace},
    ir, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileID,
};
use ruddy_compiler as ruddy;
use std::{hint::black_box, num::NonZeroUsize, time::Instant};

fn source(groups: usize, shape: &str) -> String {
    let mut text = String::from("let shared = fn x => x\n");
    for index in 0..groups {
        let dependency = match shape {
            "chain" if index > 0 => format!("value{}", index - 1),
            "recursive" => format!("value{}", (index + 1) % groups),
            _ => "shared".into(),
        };
        text.push_str(&format!(
            "let value{index} = fn x => do let changed = {dependency} x let bumped = changed + 1.0 return bumped end\n"
        ));
    }
    text
}

fn program(source: &str) -> (Mint, ir::Program) {
    let parsed = parse::parse(token::lex(source, FileID::GENERATED).tokens);
    assert!(parsed.errors.is_empty());
    let mut mint = Mint::new(Bundle::new("benchmark", Version::new(0, 0, 0)).unwrap());
    let built = ir::build(&mut mint, parsed.stmts);
    assert!(built.errors.is_empty());
    (mint, built.program)
}

fn measure(work: impl FnOnce()) -> f64 {
    let start = Instant::now();
    work();
    start.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let workers: NonZeroUsize = args.get(1).map_or("1", String::as_str).parse().unwrap();
    let groups: usize = args.get(2).map_or("32", String::as_str).parse().unwrap();
    let samples: usize = args.get(3).map_or("7", String::as_str).parse().unwrap();
    assert!(groups > 1 && samples > 0);
    let shapes = ["independent", "chain", "recursive"];
    let selected = args.get(4).map(String::as_str);
    assert!(selected.is_none_or(|shape| shapes.contains(&shape)));
    let execution = Execution::with_threads(workers).unwrap();
    for shape in shapes {
        if selected.is_some_and(|selected| selected != shape) {
            continue;
        }
        let text = source(groups, shape);
        let original = program(&text);
        let body_edit = program(&text.replacen("changed + 1.0", "changed + 2.0", 1));
        let interface_edit = program(&text.replacen("fn x => x\n", "fn x => { value: x }\n", 1));
        let mut times = [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        let mut solves = Vec::new();
        for _ in 0..samples {
            let mut session = Session::with_execution(execution.clone());
            for (index, (mint, program)) in [&original, &original, &body_edit, &interface_edit]
                .into_iter()
                .enumerate()
            {
                let before = session.solved_groups();
                times[index].push(measure(|| {
                    black_box(session.infer(mint, program, Trace::Off));
                }));
                solves.push(session.solved_groups() - before);
            }
            times[4].push(measure(|| {
                execution.run(|| {
                    let parsed = parse::parse(token::lex(&text, FileID::GENERATED).tokens);
                    let mint = Mint::new(Bundle::new("benchmark", Version::new(0, 0, 0)).unwrap());
                    black_box(ruddy::compile::compile(mint, parsed.stmts, Trace::Off).unwrap());
                })
            }));
        }
        let metrics: serde_json::Map<_, _> = ["cold", "unchanged", "body_edit", "interface_edit", "full_compile"].into_iter()
            .zip(times)
            .map(|(name, mut times)| {
                times.sort_by(f64::total_cmp);
                (name.into(), serde_json::json!({"median_ms": times[times.len()/2], "p95_ms": times[(times.len()*95).div_ceil(100)-1]}))
            }).collect();
        println!(
            "{}",
            serde_json::json!({"workers": workers.get(), "groups": groups, "samples": samples, "shape": shape, "timings": metrics, "solved_groups": solves})
        );
    }
}
