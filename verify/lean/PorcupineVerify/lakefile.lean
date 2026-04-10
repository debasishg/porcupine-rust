import Lake
open Lake DSL

-- PorcupineVerify: Lean 4 proofs for porcupine-rust hot modules.
--
-- Depends on:
--   aeneas  — Aeneas Lean library (primitives for Aeneas-translated Rust)
--   mathlib — Lean 4 math library (list/set lemmas used in proofs)
--
-- Workflow:
--   1. cd verify/
--   2. charon --crate porcupine-verify --dest llbc/
--   3. aeneas llbc/porcupine_verify.llbc -backend lean -dest lean/PorcupineVerify/
--   4. lake build
--
-- The Aeneas step generates:
--   PorcupineVerify/BitsetSpecExtracted.lean
--   PorcupineVerify/PowersetSpecExtracted.lean
--   PorcupineVerify/DfsKernelExtracted.lean
--
-- The hand-written proof files (BitsetSpec.lean etc.) import those generated
-- files and add the theorems proved on top.

package PorcupineVerify where
  name := "PorcupineVerify"

-- Mathlib4: pin to a release tag that is known-good for the Lean version in use.
-- Update the tag when bumping Lean (check https://github.com/leanprover-community/mathlib4/releases).
require mathlib from git
  "https://github.com/leanprover-community/mathlib4" @ "v4.14.0"

-- Aeneas Lean library: provides Array, Slice, Result, and other primitives that
-- Aeneas-generated code depends on.
require aeneas from git
  "https://github.com/AeneasVerif/aeneas" @ "main"

lean_lib PorcupineVerify
