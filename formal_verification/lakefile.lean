import Lake
open Lake DSL

package mangoV4Proofs where
  leanOptions := #[
    ⟨`autoImplicit, false⟩
  ]
  moreLeanArgs := #["-DwarningAsError=false"]

require mathlib from git
  "https://github.com/leanprover-community/mathlib4.git" @ "v4.15.0"

require qedgenSupport from
  "./lean_solana"

@[default_target]
lean_lib Proofs where
  roots := #[`Proofs]

lean_lib MangoState where
  roots := #[`MangoState]
