//! Benchmark: hakoniwa container spin-up/tear-down overhead.
//!
//! Measures the cost of creating a fresh kernel sandbox per bash call by comparing
//! sandboxed execution against native process spawning for identical commands.
//! The difference (sandbox - native) isolates the container overhead: namespace
//! creation, bind mount setup, and teardown.
//!
//! Run with:
//!   cargo bench -p qq-tools --bench sandbox_overhead
//!
//! Requires Linux with user namespace support for sandbox benchmarks.
//! Native baselines always run regardless of platform.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::process::Command;

use qq_tools::bash::mounts::SandboxMounts;
use qq_tools::bash::sandbox::probe_user_namespaces;

/// Native (unsandboxed) command execution via std::process::Command + /bin/sh.
/// This is the baseline: fork + exec + wait with no namespace isolation.
fn native_exec(command: &str) -> std::process::Output {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .output()
        .expect("failed to execute native command")
}

/// Commands to benchmark, from trivial to realistic.
const COMMANDS: &[(&str, &str)] = &[
    // Minimal: measures pure fork+exec+exit overhead
    ("true", "/bin/true"),
    // Simple output capture
    ("echo", "echo hello"),
    // Pipeline (shell operator handling)
    ("pipe", "echo hello | cat"),
    // Directory listing (filesystem access)
    ("ls", "ls -1"),
    // File read (I/O through sandbox mounts)
    ("cat_file", "cat Cargo.toml"),
    // Subshell + pipe (heavier shell usage)
    ("env_count", "env | wc -l"),
];

fn bench_native(c: &mut Criterion) {
    let mut group = c.benchmark_group("native_baseline");

    for &(label, cmd) in COMMANDS {
        group.bench_with_input(BenchmarkId::from_parameter(label), &cmd, |b, &cmd| {
            b.iter(|| {
                let output = native_exec(black_box(cmd));
                black_box(output);
            });
        });
    }

    group.finish();
}

fn bench_sandbox(c: &mut Criterion) {
    if !probe_user_namespaces() {
        eprintln!(
            "\n*** SKIPPING sandbox benchmarks: user namespaces not available. ***\n\
             *** Run on a Linux host with user namespace support.            ***\n\
             *** Native baselines above still provide useful data.           ***\n"
        );
        return;
    }

    // Re-import only when we know sandbox is available.
    use qq_tools::bash::sandbox::execute_kernel;

    let project_root = std::env::current_dir().expect("failed to get cwd");
    let mounts = SandboxMounts::new(project_root);

    let mut group = c.benchmark_group("sandbox");

    for &(label, cmd) in COMMANDS {
        group.bench_with_input(BenchmarkId::from_parameter(label), &cmd, |b, &cmd| {
            b.iter(|| {
                let result = execute_kernel(black_box(cmd), &mounts, 30)
                    .expect("sandbox execution failed");
                black_box(result);
            });
        });
    }

    group.finish();
}

fn bench_probe(c: &mut Criterion) {
    c.bench_function("probe_user_namespaces", |b| {
        b.iter(|| {
            let available = probe_user_namespaces();
            black_box(available);
        });
    });
}

criterion_group!(benches, bench_native, bench_sandbox, bench_probe);
criterion_main!(benches);
