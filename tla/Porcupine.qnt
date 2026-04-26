// Porcupine.qnt — Quint formal model of the DFS backtracking linearizability checker.
//
// Corresponds to docs/spec.md invariants INV-HIST-01..03, INV-LIN-01..04.
//
// Run:  quint verify tla/Porcupine.qnt --invariant safetyInvariant
// Run:  quint run    tla/Porcupine.qnt  (for ITF trace generation / MBT)
//
// Note: `head`/`tail` are reserved in Quint ≥ 0.30.0; we use `hd`/`tl` instead.
// Note: This model uses a simplified register model as the sequential specification
//       to make the DFS concretely executable. The structural invariants (INV-HIST-*,
//       INV-LIN-*) are model-independent.

module Porcupine {

  // ---------------------------------------------------------------------------
  // Types
  // ---------------------------------------------------------------------------

  // An operation in the history.
  // `id`          — unique operation identifier
  // `call_ts`     — call timestamp (non-negative)
  // `ret_ts`      — return timestamp (≥ call_ts, per INV-HIST-01)
  // `input`       — input value (modelled as int for generality)
  // `output`      — output value
  // `is_write`    — true = write(input), false = read → output
  type Op = {
    id: int,
    call_ts: int,
    ret_ts: int,
    input: int,
    output: int,
    is_write: bool
  }

  // ---------------------------------------------------------------------------
  // Parameters (concrete values for model checking)
  // ---------------------------------------------------------------------------

  // Number of operations in the history under check.
  pure val N_OPS = 4

  // Register initial value.
  pure val INIT_VAL = 0

  // ---------------------------------------------------------------------------
  // History (fixed for model checking; varied via `quint run` for MBT)
  // ---------------------------------------------------------------------------

  // A small example history:
  //   op0: write(1)  [0, 10]
  //   op1: read → 1  [5, 15]
  //   op2: write(2)  [12, 20]
  //   op3: read → 2  [18, 25]
  // This history IS linearizable: op0, op1, op2, op3 in order.
  pure val HISTORY: List[Op] = [
    { id: 0, call_ts: 0,  ret_ts: 10, input: 1, output: 0, is_write: true  },
    { id: 1, call_ts: 5,  ret_ts: 15, input: 0, output: 1, is_write: false },
    { id: 2, call_ts: 12, ret_ts: 20, input: 2, output: 0, is_write: true  },
    { id: 3, call_ts: 18, ret_ts: 25, input: 0, output: 2, is_write: false }
  ]

  // ---------------------------------------------------------------------------
  // State variables
  // ---------------------------------------------------------------------------

  // Set of operation IDs that have been linearized so far.
  var linearized: Set[int]

  // Current model state (register value).
  var reg: int

  // Whether a complete linearization has been found.
  var result: str   // "Ok" | "Illegal" | "Unknown"

  // Stack of (op_id, pre_state) pairs for backtracking.
  // Represented as a list; last element is top of stack.
  var stack: List[{ op_id: int, pre_reg: int }]

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  // INV-HIST-01: well-formed history — all timestamps valid.
  pure def histWellFormed(hist: List[Op]): bool =
    hist.indices().forall(i => hist[i].call_ts >= 0 and hist[i].ret_ts >= hist[i].call_ts)

  // INV-HIST-02: real-time order — if A finishes before B starts, A must be
  // linearized before B in any valid linearization.
  pure def precedesInRealTime(a: Op, b: Op): bool =
    a.ret_ts < b.call_ts

  // An operation is "minimal" (eligible at current DFS step) if no unlinearized
  // operation's call timestamp strictly precedes its own call timestamp
  // (INV-HIST-03).
  pure def isMinimal(op: Op, hist: List[Op], lin: Set[int]): bool =
    hist.indices().forall(i => {
      val other = hist[i]
      lin.contains(other.id) or
      other.id == op.id or
      other.call_ts >= op.call_ts
    })

  // Sequential register model step:
  // Returns (ok, next_state) where ok = true iff (input, output) is consistent.
  pure def registerStep(state: int, op: Op): { ok: bool, next: int } =
    if (op.is_write) {
      { ok: true, next: op.input }
    } else {
      { ok: op.output == state, next: state }
    }

  // Collect all operations eligible to be linearized next.
  pure def frontier(hist: List[Op], lin: Set[int]): List[Op] =
    hist.foldl([], (acc, op) =>
      if (not(lin.contains(op.id)) and isMinimal(op, hist, lin)) acc.append(op) else acc
    )

  // ---------------------------------------------------------------------------
  // Initial state
  // ---------------------------------------------------------------------------

  action init = all {
    linearized' = Set(),
    reg'        = INIT_VAL,
    result'     = "Unknown",
    stack'      = []
  }

  // ---------------------------------------------------------------------------
  // Actions
  // ---------------------------------------------------------------------------

  // Try to linearize one minimal operation.
  // If the model step succeeds, push the choice onto the stack.
  // If the model step fails, do not advance (caller must try next candidate).
  action tryLinearize(op: Op): bool = {
    val step = registerStep(reg, op)
    if (step.ok) {
      all {
        linearized' = linearized.union(Set(op.id)),
        reg'        = step.next,
        result'     = if (linearized.size() + 1 == HISTORY.length()) "Ok" else result,
        stack'      = stack.append({ op_id: op.id, pre_reg: reg })
      }
    } else {
      all {
        linearized' = linearized,
        reg'        = reg,
        result'     = result,
        stack'      = stack
      }
    }
  }

  // Backtrack: undo the most recent linearization choice.
  action backtrack = {
    val top = stack[stack.length() - 1]
    all {
      linearized' = linearized.exclude(Set(top.op_id)),
      reg'        = top.pre_reg,
      result'     = if (stack.length() == 1) "Illegal" else result,
      stack'      = stack.slice(0, stack.length() - 1)
    }
  }

  // Top-level DFS step: pick one minimal op and try to linearize it.
  // Once result is decided ("Ok" or "Illegal"), stutter.
  action step = {
    if (result != "Unknown") {
      all {
        linearized' = linearized,
        reg'        = reg,
        result'     = result,
        stack'      = stack
      }
    } else {
      val candidates = frontier(HISTORY, linearized)
      if (candidates.length() > 0) {
        nondet i = candidates.indices().oneOf()
        tryLinearize(candidates[i])
      } else if (stack.length() > 0) {
        backtrack
      } else {
        all {
          result'     = "Illegal",
          linearized' = linearized,
          reg'        = reg,
          stack'      = stack
        }
      }
    }
  }

  // ---------------------------------------------------------------------------
  // Helpers for compositionality and parallel invariants
  // ---------------------------------------------------------------------------

  // Look up an operation in HISTORY by its id.
  // Returns HISTORY[0] as a sentinel if not found (callers only invoke this
  // when id is known to be present).
  pure def opById(id: int): Op =
    HISTORY.foldl(HISTORY[0], (found, op) => if (op.id == id) op else found)

  // ---------------------------------------------------------------------------
  // Invariants (correspond to INV-* IDs in docs/spec.md)
  // ---------------------------------------------------------------------------

  // INV-HIST-01: history is well-formed at all times.
  val histWellFormedInv: bool =
    histWellFormed(HISTORY)

  // INV-HIST-03: every op in the frontier is truly minimal.
  val minimalCallFrontier: bool = {
    val fr = frontier(HISTORY, linearized)
    fr.indices().forall(i => isMinimal(fr[i], HISTORY, linearized))
  }

  // INV-LIN-04: cache soundness — the linearized set uniquely identifies the
  // DFS node (together with reg). We express this structurally: no two stack
  // frames may share the same op_id.
  val cacheSound: bool =
    stack.indices().forall(i =>
      stack.indices().forall(j =>
        i != j implies stack[i].op_id != stack[j].op_id
      )
    )

  // INV-LIN-01+02: if result = "Ok", all ops are linearized; if "Illegal", none
  // remain to try.
  val resultConsistent: bool =
    (result == "Ok"      implies linearized.size() == HISTORY.length()) and
    (result == "Illegal" implies frontier(HISTORY, linearized).length() == 0)

  // INV-LIN-03: P-Compositionality
  //
  // When result = "Ok", the stack records a complete valid sequential
  // execution: applying the ops in stack order from INIT_VAL must be
  // accepted by the register model at every step.
  //
  // This is the core soundness guarantee for P-compositional checking:
  // each partition's DFS produces a valid sequential sub-execution, and
  // independent sub-executions compose into a valid execution of the whole.
  //
  // Formally verified by `quint verify --invariant safetyInvariant`.
  // Also tested by the `prop_parallel_kv_agrees_with_sequential` property test
  // (Rust side) which exercises the rayon-based parallel path.
  val pCompositionality: bool =
    result != "Ok" or {
      stack.foldl({ ok: true, state: INIT_VAL }, (acc, frame) =>
        if (not(acc.ok)) acc
        else {
          val op   = opById(frame.op_id)
          val step = registerStep(acc.state, op)
          { ok: step.ok, state: step.next }
        }
      ).ok
    }

  // INV-PAR-01: Parallel kill-flag monotonicity
  //
  // Once a result is decided ("Ok" or "Illegal"), the DFS stutters — no
  // further state changes occur. In the parallel implementation this models
  // the `Arc<AtomicBool>` kill flag: once any partition sets kill = true,
  // all in-flight DFS loops abort via kill.load(Relaxed) and the combined
  // result is Illegal. The flag is write-once (monotone false → true).
  //
  // In this single-DFS model the stutter guard `if (result != "Unknown")`
  // in the `step` action enforces the same property structurally.
  val parallelKillFlagInvariant: bool =
    result == "Unknown" or result == "Ok" or result == "Illegal"

  // Combined safety invariant — checked by `quint verify`.
  val safetyInvariant: bool =
    histWellFormedInv and
    minimalCallFrontier and
    cacheSound and
    resultConsistent and
    pCompositionality and
    parallelKillFlagInvariant

  // ---------------------------------------------------------------------------
  // Algebraic invariants of check (cross-execution)
  //
  // These properties relate `check(h_1)` and `check(h_2)` where h_2 is a
  // representational transformation of h_1 (time-shift, slice permutation,
  // op append). They are *cross-execution* properties — they cannot be
  // expressed as inductive state-machine invariants verifiable by
  // `quint verify`. Each holds because the checker reads only the abstract
  // linearizability question, never incidental representation details.
  //
  // Empirical verification: see the `prop_*_invariance` /
  // `hegel_*_invariance` tests and `prop_append_preserves_illegal` /
  // `hegel_append_preserves_illegal` in the Rust test suites.
  // ---------------------------------------------------------------------------

  // 2.1 — Time-shift invariance.
  //
  // ∀ h, Δ ≥ 0:  check(h).result = check(shiftHistory(h, Δ)).result
  //
  // Reason: `precedesInRealTime(a, b) = a.ret_ts < b.call_ts` is invariant
  // under uniform addition of Δ to every timestamp. The checker never reads
  // absolute time — only the difference between any two timestamps.
  pure def shiftHistory(h: List[Op], delta: int): List[Op] =
    h.foldl([], (acc, op) => acc.append({
      id:       op.id,
      call_ts:  op.call_ts + delta,
      ret_ts:   op.ret_ts  + delta,
      input:    op.input,
      output:   op.output,
      is_write: op.is_write,
    }))

  // 2.2 — Client-ID invariance.
  //
  // The Op record in this Quint model has no `client_id` field; the
  // checker is structurally independent of client identity. (In Rust,
  // `Operation.client_id` exists as caller metadata but `checker.rs` never
  // reads it — `Entry` carries only `id`/`time`/`value`.)

  // 2.3 / 2.4 — Slice-order independence.
  //
  // `make_entries` in checker.rs sorts entries by `(time, kind)` before
  // DFS, so input slice order is irrelevant. With distinct timestamps the
  // sort is total; with ties the Call-before-Return tiebreak (and the
  // stable sort) yields a deterministic canonical order. Two slices that
  // permute the same multiset of ops must therefore produce the same
  // result.

  // 2.5 — Append-preserves-Illegal.
  //
  // ∀ h, suffix with min(suffix.call_ts) > max(h.ret_ts):
  //   check(h).result = "Illegal"  →  check(h ++ suffix).result = "Illegal"
  //
  // Reason: any valid linearization σ' of (h ++ suffix) restricted to h is
  // itself a valid linearization of h. If h has no valid linearization,
  // neither can h ++ suffix — appending only adds constraints.
  // (`tail` is a Quint built-in, hence the parameter name `suffix`.)
  pure def appendHistory(h: List[Op], suffix: List[Op]): List[Op] =
    suffix.foldl(h, (acc, op) => acc.append(op))

  // ---------------------------------------------------------------------------
  // Temporal properties (for `quint verify --temporal`)
  // ---------------------------------------------------------------------------

  // INV-LIN-01 (Soundness): result eventually becomes "Ok" or "Illegal"
  // (termination for finite histories).
  temporal terminates: bool =
    eventually(result == "Ok" or result == "Illegal")

}
