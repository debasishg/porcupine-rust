**Benchmarking the Rust port against the original Go implementation using the shared test infrastructure.**

Both repositories use (or can share) the exact same benchmark data from `test_data/jepsen/`, which the Go Porcupine authors originally used to claim 1,000×–10,000× speedup over Knossos (and millions× when P-compositionality applies). The Rust repo copied this data for its `go_compat` integration tests, so the histories are identical.

### Recommended Benchmarking Approach (Fair & Reproducible)

#### 1. Use the Jepsen Histories (Best Real-World Workload)
These are the largest, most representative histories in both repos.

- Go repo: `test_data/jepsen/`
- Rust repo: `test_data/jepsen/` (already present)

Typical files include:
- `jepsen-etcd.json`, `jepsen-tikv.json`, `jepsen-raft.json`, etc. (large histories from real distributed-system tests).

#### 2. Quick-and-Dirty Comparison (No Code Changes Needed)
Use **`hyperfine`** (a command-line benchmarking tool) on both binaries.

```bash
# 1. Build both
cd ~/projects/porcupine-rust
cargo build --release

cd /path/to/porcupine-go
go build -o porcupine-go ./cmd/porcupine   # or just use `go run` if you prefer

# 2. Benchmark one large history (example)
hyperfine --warmup 3 --runs 10 \
  "./target/release/porcupine-rust check --file test_data/jepsen/some-large-history.json" \
  "/path/to/porcupine-go check --file test_data/jepsen/some-large-history.json"
```

(Replace `some-large-history.json` with an actual file from `test_data/jepsen/`.)

#### 3. Proper Statistical Benchmarks (Recommended)

**For Rust (add Criterion)**  
Add this to `Cargo.toml` (dev-dependencies):
```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
```

Create `benches/linearizability.rs`:
```rust
use criterion::{criterion_group, criterion_main, Criterion};
use porcupine_rust::{check_operations, Model /* your model types */};
use std::fs;

fn bench_jepsen(c: &mut Criterion) {
    let history: Vec<Operation> = /* load JSON from test_data/jepsen/... */;
    let model = YourModel::new();   // e.g. KvModel, RegisterModel, etc.

    c.bench_function("rust_porcupine_jepsen_etcd", |b| {
        b.iter(|| {
            let _ = check_operations(&model, history.clone());
        });
    });
}

criterion_group!(benches, bench_jepsen);
criterion_main!(benches);
```

Run with:
```bash
cargo bench --bench linearizability
```

**For Go**  
The original Porcupine already has test files that load Jepsen data. You can extend `porcupine_test.go` or create a benchmark:

```go
func BenchmarkJepsenEtcd(b *testing.B) {
    ops := loadJepsenHistory("test_data/jepsen/etcd.json") // helper you add
    model := KvModel{} // or whatever model matches

    b.ResetTimer()
    for i := 0; i < b.N; i++ {
        porcupine.CheckOperations(model, ops)
    }
}
```

Run with:
```bash
go test -bench=BenchmarkJepsenEtcd -benchmem -count=10
```

#### 4. Apples-to-Apples Comparison Tips
- **Control parallelism**:
  - Rust: The checker uses `rayon::par_iter` by default. You can add a `sequential: bool` flag to `check_operations` (or temporarily comment out the parallel path) for fair comparison.
  - Go: Porcupine’s default is single-threaded; you can enable goroutine parallelism via its internal options if you want to match Rust.
- **Timeout**: Disable or set to `None`/`0` in Rust so it doesn’t interfere.
- **Warm-up & repeats**: Use `--warmup` in hyperfine or Criterion’s default statistics.
- **Memory usage**: Add `--mem` to hyperfine or use `time`/`/usr/bin/time -v`.
- **Same machine**: Run both back-to-back on the same hardware.

### Expected Outcome
Based on the architecture review:
- The Rust port already has several low-level wins (bitset for remaining ops, arena-based history, better hashmap collision handling, no GC).
- On large partitioned histories you should see **Rust noticeably faster** (often 1.5–4× depending on the workload) once the visualizer and other WIP pieces are not active.
- On tiny histories the difference will be smaller.

Would you like me to:
- Give you the exact `loadJepsenHistory` helper code for both languages?
- Write a complete ready-to-paste Criterion benchmark for the Rust side?
- Suggest a specific Jepsen file that gives the most interesting comparison?

Just say the word and I’ll drop the code snippets tailored to your repo.