# RULES OF ENGAGEMENT & REPOSITORY GOVERNANCE
**System:** AetherStream Engineering Operations  
**Compliance Level:** MANDATORY  
**Revision:** 2.0.0-PROD  

---

## 1. WORKSPACE MODIFICATION RULES

1. **Deterministic Single-Subphase Execution:**
   - Work MUST proceed one subphase at a time in strict chronological and dependency order:
     $$\text{STAGE 1} \to \text{STAGE 2} \to \text{STAGE 3} \to \text{STAGE 4} \to \text{STAGE 5} \to \text{STAGE 6}$$
   - Skipping subphases or jumping across stages without prior subphase completion is strictly forbidden.

2. **Atomic Rollback & State Protection:**
   - Every mutation of `state.tracker.json` must use a write-checkpoint pattern (write to `.tmp` file, verify JSON parse integrity, replace atomically).
   - If an automated test fails, the state tracker MUST remain unchanged, preserving the previous stable state.

3. **Append-Only Integrity on Audit Files:**
   - Under NO circumstance may `CHANGELOG.md` or historical entries in `TODO.md` be truncated, rewritten, or deleted.
   - Any script modifying these files must verify:
     $$\text{Length}(\text{New}) > \text{Length}(\text{Old}) \quad \land \quad \text{StartsWith}(\text{NewContent}, \text{OldContent})$$

4. **Conventional Commits with Subphase Anchor:**
   - Pattern: `<type>(stage-<X>.<Y>): <subject> [SUBPHASE-<X>.<Y>.<Z>]`
   - Example: `feat(stage-1.1): probe shell environment and hardware blocks [SUBPHASE-1.1.1]`

5. **Security Boundaries & Isolation:**
   - Never commit private keys, WireGuard secrets, or Cloudflare tokens.
   - All tests run against ephemeral mock directories in `scratch/`.
