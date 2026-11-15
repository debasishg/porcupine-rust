# Hegel vs proptest in porcupine-rust

This project keeps both [`proptest`](https://docs.rs/proptest) and
[Hegel](https://hegel.dev) (`hegeltest`) as dev-dependencies. They cover
overlapping ground but have orthogonal failure modes; this document records
why we picked that arrangement and when each one is the right tool.

The two test files mirror each other: `tests/property_tests.rs` (proptest) and
`tests/hegel_properties.rs` (Hegel) test the same INV-* invariants from
`docs/spec.md`. Shared models and history builders live in `tests/common/`.

---

## TL;DR

- **proptest** wins on speed, stability, and zero runtime footprint. Use it
  for the inner-loop properties run on every CI build.
- **Hegel** wins on shrinker quality, stateful testing, and the path to
  running under Antithesis's deterministic simulator. Use it when you need a
  *minimal* counterexample, when state-machine coverage matters, or when
  porcupine's spec gets cross-validated against a non-Rust implementation.
- Keep both. When proptest passes and Hegel fails (or vice versa), that is
  information.

---

## Where Hegel wins

### Shrinker quality

Hypothesis's IR-based shrinker is the state of the art in property-based
testing. It shrinks across draws, reorders, and reliably finds minimal
counterexamples even for nested structures. proptest's shrinker is
value-based and routinely gets stuck on local minima.

For a linearizability checker this matters: when a property fails, the
failing input is a tangled `Vec<Operation<…, …>>` whose `call` and
`return_time` interleave in subtle ways. A smaller, cleaner counterexample
is the difference between "I can see the bug in 30 seconds" and "I'll be
puzzling over this for an hour."

### Stateful testing as a first-class citizen

`#[hegel::state_machine]` is built in, integrates with the same shrinker, and
is what powers `hegel_incremental_register_is_linearizable` in this repo.
proptest's stateful story
([`proptest-state-machine`](https://docs.rs/proptest-state-machine)) is a
separate crate, less polished, and shrinks worse.

### Richer generator library out of the box

Hegel ships `regex`, `dates`, `ip_addresses`, `emails`, `urls`, `hashmaps`,
`hashsets`, `arrays`, `from_regex`, `characters`. proptest can build all of
these via combinators, but you have to write the combinator each time.

### Path to Antithesis

Hegel tests can be lifted into Antithesis's deterministic simulator without
rewriting. Each `tc.draw` call becomes a controlled choice point the
simulator drives. If porcupine ever runs under Antithesis, the existing
Hegel suite is free leverage; the proptest suite would have to be rewritten.

### Cross-language consistency

The same Hegel protocol is available in Go, C++, TypeScript, and OCaml.
Mostly irrelevant for a single-language project, but it matters if porcupine
is ever cross-validated against a Go reference implementation: the Hegel
generators on both sides produce comparable inputs.

---

## Where proptest wins

### Speed

Both run in-process. (Through `hegeltest 0.8.x`, Hegel talked to a Python
`hegel-core` server over a local socket, so every `tc.draw` was a
round-trip; from `0.42` onward a native Rust port of the engine is the only
backend.)

Concrete numbers from this project (Rust 1.100, `hegeltest 0.52`, debug
build):

| Suite | Tests | Wall time |
| --- | --- | --- |
| `cargo test --test property_tests` | 51 | ~0.11 s |
| `cargo test --test hegel_properties` | 44 | ~0.27 s |

The gap that used to be two orders of magnitude (~0.05 s vs ~2.2 s under
the Python sidecar) is now a small constant factor.

### Zero extra runtime

proptest is pure Rust, vendored as a normal cargo dependency.

Hegel is now pure Rust as well. The old `uv` download, Python sidecar and
`.hegel/server.*.log` files went away with the native engine. The sidecar
was fragile in practice: under `hegeltest 0.8.x`, `uv` resolved
`hegel-core 0.4.7` against a newer Hypothesis that no longer had
`ConjectureRunner.interesting_examples`, and every Hegel test hung until
we upgraded. Hegel still writes an example database to `.hegel/`.

### Stability

proptest is mature and stable; the API has not meaningfully changed in
years.

`hegeltest` is still pre-1.0 and changes quickly. Going from 0.8 to 0.52
broke two APIs used here: `#[hegel::composite]` generators now take
`&TestCase`, and `hegel::stateful::run(m, tc)` became the builder
`hegel::stateful::machine(m).run(tc)`. Expect to touch the Hegel suite on
upgrades.

### Ecosystem

proptest has wider integration: serde-aware `arbitrary` derives, more
third-party strategies, well-trodden idioms, easy `prop_assert!` and
`prop_assert_eq!` macros. The Rust community knows proptest.

### CI footprint

No `~/.cache/hegel` to manage, no `.hegel/` log directory to gitignore, no
first-run download to absorb in CI cold starts, no Python runtime
to keep alive.

---

## How they complement each other in this repo

The two suites assert the same INV-* invariants but generate inputs
differently and shrink differently. That redundancy is the point: a bug
that slips past one engine's generator/shrinker may be caught by the
other's.

| Use case | Pick |
| --- | --- |
| Default CI run (`cargo test`) | Both — they're cheap enough together |
| Iterating on a checker change locally | proptest — fastest feedback |
| Investigating a real failure | Hegel — better shrinking |
| Adding a stateful invariant | Hegel — `#[hegel::state_machine]` |
| Benchmarking the checker on huge histories | Neither — use `criterion` |
| Running under Antithesis later | Hegel — direct lift |

If we ever had to drop one, we would drop Hegel — it is the optional layer.
But while we have both, the cost of keeping them is low (shared models live
in `tests/common/`) and the bug-finding power is strictly larger than
either alone.

---

## References

- [Hypothesis, Antithesis, synthesis (Antithesis blog)](https://antithesis.com/blog/2026/hegel/)
- [Hegel website](https://hegel.dev)
- [`hegel-rust` on GitHub](https://github.com/hegeldev/hegel-rust)
- [proptest book](https://proptest-rs.github.io/proptest/)
- Section 4 (proptest) and Section 4b (Hegel) of `docs/all_tests.md`
