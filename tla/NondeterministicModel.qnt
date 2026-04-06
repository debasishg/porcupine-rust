// NondeterministicModel.qnt — Quint formal model of the NondeterministicModel trait
// and the PowerSetModel adapter (src/model.rs).
//
// Corresponds to docs/spec.md invariant INV-ND-01.
//
// Run:  quint verify tla/NondeterministicModel.qnt --invariant safetyInvariant
// Run:  quint run    tla/NondeterministicModel.qnt  (simulation)
//
// This module is independent of Porcupine.qnt.  It models only the
// NondeterministicModel / PowerSetModel layer, not the DFS algorithm.
//
// Concrete model used: NdRegister — a single-integer register with a *lossy*
// write.  A write of value `v` from state `s` may update to `v` (write
// succeeds) or remain at `s` (write is lost).  A read must return the exact
// current register value.
//
//   ndStep(s, Write(v), None)    → { v, s }    (two valid successors)
//   ndStep(s, Write(v), None)    → { s }       (when v == s, deduplicated)
//   ndStep(s, Read,     Some(o)) → { s }       (when o == s)
//   ndStep(s, Read,     Some(o)) → {}          (when o ≠ s — rejection)

module NondeterministicModel {

  // ---------------------------------------------------------------------------
  // Types
  // ---------------------------------------------------------------------------

  // Represent Input as a tagged union via a record.
  //   op = 0 → Read  (value ignored)
  //   op = 1 → Write (value = value to write)
  type Input = { op: int, value: int }

  // ---------------------------------------------------------------------------
  // Parameters
  // ---------------------------------------------------------------------------

  // Initial register value.
  pure val INIT_VAL: int = 0

  // ---------------------------------------------------------------------------
  // NondeterministicModel step function (src/model.rs NondeterministicModel::step)
  //
  // Returns the set of all valid successor states for a given (state, input, output) triple.
  // An empty set means rejection.
  //
  // input.op == 0 → Read:  output must equal state; no state change.
  // input.op == 1 → Write: register may update to input.value or remain unchanged (lossy).
  //   output is None for writes — modelled as output == -1 (a sentinel).
  // ---------------------------------------------------------------------------

  pure def ndStep(state: int, input: Input, output: int): Set[int] =
    if (input.op == 1) {
      // Write: two possible successors (deduplicated when input.value == state)
      Set(input.value, state)
    } else {
      // Read: accepted only when observed output matches current state
      if (output == state) Set(state) else Set()
    }

  // ---------------------------------------------------------------------------
  // Power-set step function (src/model.rs PowerSetModel::step)
  //
  // Fans out over all states in the current power-state, collects all
  // successor states, and deduplicates via set union.
  // Returns an empty set when no state produced a valid successor (rejection).
  // ---------------------------------------------------------------------------

  pure def powerSetStep(powerState: Set[int], input: Input, output: int): Set[int] =
    powerState.fold(Set(), (acc, s) => acc.union(ndStep(s, input, output)))

  // ---------------------------------------------------------------------------
  // State variables — model a single history step
  // ---------------------------------------------------------------------------

  // Current power-state (set of all states the system could be in).
  var powerState: Set[int]

  // Whether the most recent step was accepted.
  var accepted: bool

  // ---------------------------------------------------------------------------
  // Initial state: power-state = singleton of the initial register value.
  // Mirrors PowerSetModel::init (deduplicate(M::init())).
  // ---------------------------------------------------------------------------

  action init = all {
    powerState' = Set(INIT_VAL),
    accepted'   = true
  }

  // ---------------------------------------------------------------------------
  // Actions: apply a Read or Write step to the current power-state.
  // ---------------------------------------------------------------------------

  // Apply a Write of value `v`.  The output sentinel is -1 (writes have no output).
  action applyWrite(v: int) = {
    val next = powerSetStep(powerState, { op: 1, value: v }, -1)
    all {
      powerState' = next,
      accepted'   = next.size() > 0
    }
  }

  // Apply a Read expecting output `o`.
  action applyRead(o: int) = {
    val next = powerSetStep(powerState, { op: 0, value: 0 }, o)
    all {
      powerState' = next,
      accepted'   = next.size() > 0
    }
  }

  // Non-deterministic step: choose any action.
  action step =
    any {
      nondet v = Set(-5, -1, 0, 1, 5).oneOf()
      applyWrite(v),

      nondet o = Set(-5, -1, 0, 1, 5).oneOf()
      applyRead(o),
    }

  // ---------------------------------------------------------------------------
  // INV-ND-01: Power-Set Reduction Soundness
  //
  // Three structural properties that must hold at every state:
  //
  // (a) Empty-set rejection: if powerState is empty then accepted = false.
  //     (After an invalid step, the power-state collapses to empty and the
  //      adapter returns None.)
  //
  // (b) Non-empty acceptance: if accepted = true then powerState is non-empty.
  //     (The adapter only returns Some(s) when at least one successor exists.)
  //
  // (c) Deduplication: powerState contains no duplicate values (trivially
  //     satisfied because Quint's Set is inherently deduplicated — this mirrors
  //     the Rust `deduplicate` helper using PartialEq).
  //
  // Together these three properties capture INV-ND-01 from docs/spec.md.
  // ---------------------------------------------------------------------------

  val powerSetSoundnessInv: bool =
    // (a) empty power-state → rejected
    (powerState.size() == 0 implies not(accepted)) and
    // (b) accepted → non-empty power-state
    (accepted implies powerState.size() > 0)

  // Combined safety invariant — checked by `quint verify`.
  val safetyInvariant: bool =
    powerSetSoundnessInv

  // ---------------------------------------------------------------------------
  // Additional point-wise properties (used as sanity assertions in quint run)
  // ---------------------------------------------------------------------------

  // After a write of value v, v is always in the power-state (write may succeed).
  // This is a liveness-adjacent property; modelled as a pure def for simulation.
  pure def writeAlwaysProducesSuccessor(state: int, v: int): bool =
    ndStep(state, { op: 1, value: v }, -1).contains(v)

  // A read of the current (singleton) state always succeeds.
  pure def readCurrentStateAlwaysAccepted(state: int): bool =
    ndStep(state, { op: 0, value: 0 }, state) == Set(state)

  // A read of a wrong value is always rejected.
  pure def readWrongValueAlwaysRejected(state: int, wrong: int): bool =
    wrong != state implies ndStep(state, { op: 0, value: 0 }, wrong) == Set()

  // Degenerate case: a NondeterministicModel that always returns exactly one
  // successor behaves identically to a deterministic Model.
  // Here we verify this for the Read case (single successor if output matches).
  pure def deterministicReadIsEquivalent(state: int, o: int): bool =
    val nd = ndStep(state, { op: 0, value: 0 }, o)
    if (o == state) nd == Set(state) else nd == Set()

}
