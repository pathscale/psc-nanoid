# psc-nanoid Review: Full (also covers bounded-spsc-queue)

**Date:** 2026-07-27
**Scope:** two crates, read in full including tests and manifests.
- `/Users/revenge/code/psc-nanoid`: `src/lib.rs`, `src/alphabet.rs`, `src/packed.rs`, `Cargo.toml`, `Cargo.lock`, `README.tpl`, `.github/workflows/ci.yaml`, `docs/release_procedure.md`, `clippy.toml`, `rustfmt.toml`
- `/Users/revenge/code/bounded-spsc-queue`: `src/lib.rs`, `benches/benchmark.rs`, `Cargo.toml`, `README.md`, `.travis.yml`
- Read-only sweep of `/Users/revenge/code` for consumers of both crates.
- Read `/Users/revenge/code/crates-io-metadata-review.md` (worktable + endpoint-libs). This document **extends** it; see "Extending the crates.io metadata review" below.

**Commits:** `psc-nanoid` c31fd7d · `bounded-spsc-queue` edeb064. Both trees clean (`git status` empty), no work in progress at risk.
**Reviewer slice:** full, sole reviewer for both crates. Sibling reviews cover the consumer repos (pays.online-backend, WorkTable, nofilter.io-backend, api.support.cafe and others).

---

## Summary

- **psc-nanoid's randomness is correct.** `rand::thread_rng()` is a ChaCha12 CSPRNG, seeded from the OS and periodically reseeded; there is no fallback path to a weak or deterministic source. The byte-to-character mapping goes through `rand::distributions::Uniform`, which is rejection-sampled, so the classic nanoid modulo-bias bug is **absent**. Default entropy is 126 bits; the family's `Nanoid<16, Base62Alphabet>` is 95.3 bits, which is fine for the session identifiers it backs.
- **But psc-nanoid has one Critical soundness hole**: the `rkyv` `Deserialize` impl builds a `Nanoid` from arbitrary bytes with **zero validation**, and `Nanoid::as_str` is `from_utf8_unchecked`. Deserializing a corrupted or attacker-influenced archive and calling `.as_str()` is instant UB. Confirmed empirically. Nine repos in this family enable the `rkyv` feature, and several persist those IDs to disk and S3 through WorkTable.
- **psc-nanoid's CI is red and has been since the rename.** 34 of 36 doctests fail to compile because every example says `use nid::…` while the crate is `psc_nanoid`. `cargo test --all-features` fails today.
- **`PackedNanoid<N, B, A>` lets you pass a wrong `B` and silently get a different ID back.** Every consumer in the family writes `B` by hand.
- **bounded-spsc-queue's `unsafe` is mostly well-reasoned and the SPSC contract is genuinely enforced by the type system** (`Producer`/`Consumer` are neither `Clone` nor `Sync`, compile-verified). This is *not* the WorkTable pattern: no hand-written `Sync` gives away unguarded shared access. But there is one memory-ordering bug (a `Relaxed` load where an `Acquire` is required), one reference-validity bug, and no loom/miri/CI of any kind.
- **bounded-spsc-queue is an orphan fork.** Nothing in `/Users/revenge/code` depends on it, and it cannot be published: name, version, repository, homepage and documentation URLs all still point at upstream `polyfractal/bounded-spsc-queue` 0.4.0.

**Top three things to do**

1. Validate in psc-nanoid's `rkyv::Deserialize` (P-01). One function, unblocks a UB path that is live in production persistence.
2. Fix psc-nanoid's doctests so CI is green again (P-02), then add the `const` size check to `PackedNanoid::pack` (P-03).
3. In bounded-spsc-queue, change `try_push`'s `head` load to `Acquire` (B-01) and stop building a `&mut T` to uninitialized memory (B-02), then add a loom test and a CI workflow (B-05).

---

# Part 1: `psc-nanoid`

Version 3.1.1. A fork of [`nid`](https://crates.io/crates/nid) 3.0.0 (upstream `ciffelia/nid`) with three local additions: the `packed` module (39d7890), a `Default` impl (e2b2b72), and the `rkyv` feature. All three findings below that are rated Medium or worse are in the **fork-local** code, not upstream's.

## Findings

### [P-01] `rkyv::Deserialize` constructs a `Nanoid` from unvalidated bytes; `as_str` then hits `from_utf8_unchecked`

- **ID:** `psc-nanoid-full-01`
- **Severity:** Critical
- **Category:** Security / Correctness (unsoundness)
- **Confidence:** High (reproduced)
- **Location:** `/Users/revenge/code/psc-nanoid/src/lib.rs:368-378` (the impl), `/Users/revenge/code/psc-nanoid/src/lib.rs:342-345` (`as_str`). Sibling, lower severity: `/Users/revenge/code/psc-nanoid/src/packed.rs:293-303`.
- **What:** The whole type is built around the invariant "`inner` is always ASCII and always drawn from `A`'s alphabet". `try_from_bytes` enforces it (`lib.rs:315-328`), `new_with` enforces it via `assert_all_ascii` (`alphabet.rs:107-113`), and the serde `Deserialize` impl enforces it by routing through `try_from_str` (`lib.rs:474-482`). The rkyv impl does not:

  ```rust
  impl<const N: usize, A: Alphabet, D: rkyv::rancor::Fallible + ?Sized>
      rkyv::Deserialize<Nanoid<N, A>, D> for [u8; N]
  {
      fn deserialize(&self, _: &mut D) -> Result<Nanoid<N, A>, D::Error> {
          Ok(Nanoid { inner: *self, _marker: PhantomData })   // no check at all
      }
  }
  ```

  `Archived = [u8; N]` (`lib.rs:350`), and `CheckBytes` for `[u8; N]` accepts every bit pattern, so rkyv's validating `from_bytes` does not help either. `as_str` is then:

  ```rust
  pub const fn as_str(&self) -> &str {
      // SAFETY: all characters are ASCII.
      unsafe { std::str::from_utf8_unchecked(&self.inner) }
  }
  ```

  The SAFETY comment states an invariant that this deserialization path does not uphold.
- **Reproduced:** feeding `[0xFF; 16]` through `rkyv::to_bytes` and back out as `Nanoid<16, Base62Alphabet>` returns `Ok`, with `inner == [255; 16]`. That value is not valid UTF-8, so every subsequent `as_str`, `Display`, `Debug`, `AsRef<str>`, `String::from` and serde `Serialize` on it is UB.
- **Why it matters:** `rkyv` is enabled by nine crates in this family (`WorkTable/Cargo.toml:42`, `DataBucket/Cargo.toml:20`, `honey_id-types/Cargo.toml:43`, `api.support.cafe/Cargo.toml:24`, `api.honey.id-backend`, `auth.honey.id-backend`, `nofilter.io-backend`, `pays.online-backend/Cargo.toml:11`, and WorkTable's worktree copy). `honey_id-types/src/types/id_entities.rs:11-16` derives `rkyv::{Archive, Deserialize, Serialize}` on `AppPublicId(Nanoid<16, Base62Alphabet>)`, and those rows are persisted by WorkTable to local disk and to S3. So the untrusted-input path is not hypothetical: a truncated write, a bit-flipped S3 object, a snapshot written by an older/beta build, or anyone who can influence the persistence bucket produces a `Nanoid` that is UB to read. It is also a memory-disclosure primitive: `from_utf8_unchecked` on garbage handed to a formatter or an HTTP response body reads and emits whatever is there.
- **Fix:** Route the rkyv path through the same validator as everything else. Mechanical.

  ```rust
  impl<const N: usize, A: Alphabet, D> rkyv::Deserialize<Nanoid<N, A>, D> for [u8; N]
  where
      D: rkyv::rancor::Fallible + ?Sized,
      D::Error: rkyv::rancor::Source,
  {
      fn deserialize(&self, _: &mut D) -> Result<Nanoid<N, A>, D::Error> {
          Nanoid::try_from_bytes(self).map_err(rkyv::rancor::Source::new)
      }
  }
  ```

  Adding the `D::Error: Source` bound is technically a breaking change for anyone using an infallible deserializer, but in practice every consumer here uses `rkyv::rancor::Error`. If you want to avoid that, the alternative is to make `as_str` non-`unsafe` (`str::from_utf8(&self.inner).expect(...)`, or `debug_assert!` plus the unchecked call) so the invariant break degrades to a panic rather than UB. Do at least one of the two.

  Do the same for `PackedNanoid` (`packed.rs:293-303`) for consistency, though that one is not a soundness bug: `unpack` validates every index (`packed.rs:259-264`), so a bad archive only yields a wrong-but-safe value.
- **Effort:** S
- **Blast radius:** one impl in psc-nanoid. No consumer source changes. Requires a 3.1.2 (or 3.2.0 if you add the `Source` bound) release and a bump in the nine dependents.

---

### [P-02] Every doctest is broken; `cargo test --all-features` fails on `main`

- **ID:** `psc-nanoid-full-02`
- **Severity:** High
- **Category:** Docs / Maintainability
- **Confidence:** High (reproduced)
- **Location:** `/Users/revenge/code/psc-nanoid/src/lib.rs` (24, 31, 41, 49, 57, 64, 118, 127, 137, 151, 166, 217, 237, 277, 309, 334, 496), `/Users/revenge/code/psc-nanoid/src/alphabet.rs` (13, 43, 138-186), `/Users/revenge/code/psc-nanoid/src/packed.rs` (8, 106, 136, 161, 185, 370)
- **What:** The fork renamed the package from `nid` to `psc-nanoid` without a `[lib] name = "nid"` entry, so the library crate is `psc_nanoid`. Every doc example still says `use nid::Nanoid;`. Result:

  ```
  $ cargo test --all-features
  ...
  error[E0433]: cannot find module or crate `nid` in this scope
  test result: FAILED. 2 passed; 34 failed; ...
  error: doctest failed, to rerun pass `--doc`
  ```

  The 37 unit tests all pass; it is purely the documentation examples. `lib.rs:19` also still tells users to add `nid = "3.0.0"` to their `Cargo.toml`.
- **Why it matters:** Three compounding costs. (1) The `test` and `test-msrv` jobs in `.github/workflows/ci.yaml` run `cargo test --no-fail-fast --workspace --all-features`, so CI is red, so the `release` job (which `needs: test`) can never fire, so **the automated publish path is dead** and 3.1.1 must have been published some other way. (2) The docs.rs page for the crate shows copy-paste examples that do not compile, for a crate nine internal repos depend on. (3) A permanently-red CI trains everyone to ignore it, which is how the next real regression gets through.
- **Fix:** `sed -i 's/\bnid::/psc_nanoid::/g'` across `src/`, fix `lib.rs:19` to `psc-nanoid = "3.1"`, then regenerate `README.md` with `cargo readme > README.md` (the `readme` CI job checks this). While there, `README.tpl` still points every link and badge at upstream: `https://docs.rs/nid/...` in ten link definitions, `ciffelia/nid` for the CI badge, `crates.io/crates/nid` and `docs.rs/nid` for the others. Mechanical.
- **Effort:** S
- **Blast radius:** psc-nanoid only. No API change.

---

### [P-03] `PackedNanoid<N, B, A>` accepts a wrong `B` and silently returns a different ID

- **ID:** `psc-nanoid-full-03`
- **Severity:** High
- **Category:** Correctness / Design
- **Confidence:** High (reproduced in both debug and release)
- **Location:** `/Users/revenge/code/psc-nanoid/src/packed.rs:122-125` (the type), `:213-240` (`pack_impl`), `:242-270` (`unpack_impl`), specifically `:228`, `:235` and `:256`
- **What:** `B` is a free const parameter with no compile-time relationship to `N` and `A::PACK_BITS`. `pack_impl` guards its writes with `&& dst_idx < B` (line 228) and `if bits_in_buffer > 0 && dst_idx < B` (line 235), so when `B` is too small it **silently stops writing and returns `Ok`**. `unpack_impl` then runs off the end of `src` and reaches `bits_in_buffer -= pack_bits` (line 256) with `bits_in_buffer < pack_bits`.

  Measured with `Nanoid<16, Base62Alphabet>` = `"abcdefghijklmnop"` and a deliberately wrong `B = 4`:

  | build | `pack` | `unpack` |
  |---|---|---|
  | debug | `Ok([105, 183, 29, 121])` | **panic** `attempt to subtract with overflow` at `packed.rs:256` |
  | release | `Ok([105, 183, 29, 121])` | `Ok("abcdeAAAAABptx15")`, a **different, well-formed, parseable ID** |

  Oversized `B` is harmless (`B = 20` round-trips correctly), so only the undersized direction bites.
- **Why it matters:** Every consumer spells `B` out by hand, from a mental `ceil(N * bits / 8)`: `honey_id-types/src/types/id_entities.rs:47,54` (`PackedNanoid<16, 12, Base62Alphabet>`), `api.support.cafe/src/id_types.rs:5`, `auth.honey.id-backend/src/db/util.rs`, `WorkTable/src/mem_stat/mod.rs:14`, `DataBucket/src/util/sized.rs:4`. One typo, or one change of `N` or of the alphabet without recomputing `B`, and IDs silently collide and truncate at the storage layer with no error anywhere. The release-mode behaviour is the dangerous one: a wrong-but-valid ID flows onward as if it were correct. Note that Base62 needs 6 bits per character but only uses 62 of the 64 codes, so the mismatch will not even reliably show up as an `InvalidIndex`.

  The macro that computes `B` correctly already exists (`packed_nanoid_type!`, `packed.rs:377-388`) and `api.support.cafe/src/id_types.rs:5` uses it, but nothing forces you to.
- **Fix:** Make the wrong `B` a compile error. `A::PACK_BITS` is an associated const, so a `const` block inside the generic function works on current stable:

  ```rust
  pub fn pack(nanoid: &Nanoid<N, A>) -> Result<Self, PackError> {
      const { assert!(B == (N * A::PACK_BITS).div_ceil(8), "PackedNanoid: B must be ceil(N * PACK_BITS / 8); use packed_nanoid_type!()") };
      ...
  }
  ```

  Put the same assertion in `unpack`, `as_bytes` and `from_bytes_unchecked` (or, better, in a single private `const fn check()` called from all four). Separately, make `unpack_impl` defensive: `if bits_in_buffer < pack_bits { return Err(...) }` before line 256, so even a future path that dodges the const check errors rather than corrupting.

  The `// TODO: Remove this when #76560 (generic_const_exprs) will be stabilized` at `packed.rs:361` is the right long-term note; the const assertion is the correct interim.
- **Effort:** S
- **Blast radius:** psc-nanoid plus a recompile of the six repos naming `PackedNanoid` directly. If any of them currently has a wrong `B`, it stops compiling, which is the point. I did not audit each consumer's `B` value; `honey_id-types`'s `<16, 12>` and `api.support.cafe`'s macro use are both correct.

---

### [P-04] The two MSRV CI jobs cannot work: the manifest has no `rust-version`

- **ID:** `psc-nanoid-full-04`
- **Severity:** Medium
- **Category:** Maintainability
- **Confidence:** High (verified)
- **Location:** `/Users/revenge/code/psc-nanoid/.github/workflows/ci.yaml` (`check-msrv` and `test-msrv` jobs), `/Users/revenge/code/psc-nanoid/Cargo.toml`
- **What:** Both jobs do `export RUST_TOOLCHAIN_MSRV=$(cargo read-manifest | jq -r '.rust_version')` then `rustup toolchain install "$RUST_TOOLCHAIN_MSRV"`. `cargo read-manifest` on this manifest yields `rust_version: null`, so `jq -r` emits the literal string `null` and rustup is asked to install a toolchain called `null`.
- **Why it matters:** Two of the eight `needs:` gates on the release job are structurally broken, and the crate has no declared MSRV at all, so consumers cannot know what it needs. `packed.rs:383` uses `div_ceil` (stable 1.73) and `packed.rs:80` uses `ilog2` (stable 1.67), so the real floor is at least 1.73.
- **Fix:** Add `rust-version = "1.73"` (verify by running `cargo hack check --feature-powerset --no-dev-deps` on 1.73) to `Cargo.toml`. Optionally make the jobs fail loudly on a null: `[ "$RUST_TOOLCHAIN_MSRV" != "null" ] || { echo "no rust-version in Cargo.toml"; exit 1; }`.
- **Effort:** S
- **Blast radius:** CI only.

---

### [P-05] `new_with` takes any `Rng`, not a `CryptoRng`

- **ID:** `psc-nanoid-full-05`
- **Severity:** Medium
- **Category:** Security / API design
- **Confidence:** High
- **Location:** `/Users/revenge/code/psc-nanoid/src/lib.rs:243`
- **What:** `pub fn new_with(mut rng: impl rand::Rng) -> Self`. Nothing stops a caller passing `SmallRng` (xoshiro, trivially predictable from a handful of outputs), `StdRng::seed_from_u64(...)`, or a `StepRng`. The docs at `lib.rs:227-233` document only the panic conditions; they never say the RNG must be cryptographically secure. Contrast `new()` (`lib.rs:223-225`), which uses `rand::thread_rng()` and whose doc line says "seeded by the system".
- **Why it matters:** This crate mints session identifiers (`api.support.cafe/src/service/session.rs:47`: `Nanoid::<16, Base62Alphabet>::new()` for a session ID). An ID generator whose "bring your own RNG" door is not marked is a footgun with a security payload: a developer reaching for `new_with` to get determinism in a test can leave it in a seeded-RNG code path and every ID becomes predictable, with no compiler or reviewer signal. The generic `impl Rng` also means the mistake is invisible at the call site.

  This is the same class of defect as the sibling finding in pays.online-backend, where `fastrand::alphabetic()` is used for payment links; see "Cross-repo" below. The difference is that psc-nanoid's *default* is correct, so this is a latent hazard, not an active one. I found no consumer in `/Users/revenge/code` calling `new_with` at all.
- **Fix:** Tighten the bound to `impl rand::Rng + rand::CryptoRng`. `rand 0.8`'s `ThreadRng`, `StdRng`, `OsRng` and `ChaCha*Rng` all satisfy it; `SmallRng` and `StepRng` do not, which is the intent. This is a breaking change for anyone passing a non-crypto RNG on purpose, so if you want it non-breaking: keep `new_with` and add `new_with_crypto`, deprecating the former, or at minimum add a `# Security` doc section on `new_with` saying the RNG must be a CSPRNG for any ID that must be unguessable.
- **Effort:** S
- **Blast radius:** psc-nanoid, plus a major/minor bump. No current internal caller.

---

### [P-06] crates.io metadata: `description = "Fork of nid crate"`, no `documentation`, no `homepage`, `rkyv` feature undocumented

- **ID:** `psc-nanoid-full-06`
- **Severity:** Medium
- **Category:** Docs
- **Confidence:** High
- **Location:** `/Users/revenge/code/psc-nanoid/Cargo.toml:5`, `:1-11`; `/Users/revenge/code/psc-nanoid/src/lib.rs:80-84`
- **What:** Four separate things.

  1. `description = "Fork of nid crate"` is the crates.io search blurb and the docs.rs subtitle. It says nothing about what the crate does, and it points at a crate the reader then has to go look up.
  2. No `documentation`, no `homepage`. `keywords` and `categories` **are** present (unlike the two crates in the earlier metadata review), but `"uuid"` is misleading (this is not a UUID implementation) and `"nano"` is dead weight next to `"nanoid"`. `"url-safe"` and `"identifier"` would earn their slots.
  3. No `rust-version` (see P-04) and no `readme` key (defaults to `README.md`, which exists, so this one is cosmetic).
  4. The Features list at `lib.rs:80-84` documents `serde`, `zeroize` and `packed` but **not `rkyv`**, which is the feature nine repos actually turn on and the one with the Critical bug above. Note also that `zeroize` and `serde` are implicit features (optional deps without a `dep:` reference), while `rkyv` and `packed` are explicit in `[features]`; that asymmetry is fine but worth a comment.
- **Fix:**

  ```toml
  description = "Type-safe Nano ID generation and parsing: fixed-length, alphabet-parameterised, URL-safe identifiers with optional serde, rkyv and bit-packed representations."
  documentation = "https://docs.rs/psc-nanoid"
  homepage = "https://github.com/pathscale/psc-nanoid"
  keywords = ["nanoid", "identifier", "url-safe", "id", "random"]
  rust-version = "1.73"
  ```

  Keep `categories = ["data-structures", "parser-implementations"]`; both are valid slugs and both fit. Add an `### rkyv` entry to the Features list in `lib.rs`, and once P-01 is fixed, say explicitly that rkyv deserialization validates against the alphabet.

  Keep `[package.metadata.docs.rs] all-features = true` + `rustdoc-args = ["--cfg", "doc_auto_cfg"]` (`Cargo.toml:13-15`). That is correct and is more docs.rs configuration than either crate in the earlier review has.
- **Effort:** S
- **Blast radius:** manifest and one doc comment. Lands on the crates.io page with the next publish; no version bump required on its own.

---

### [P-07] `PackedNanoid::from_bytes_unchecked` is `unsafe` but has no safety invariant

- **ID:** `psc-nanoid-full-07`
- **Severity:** Low
- **Category:** API design
- **Confidence:** High
- **Location:** `/Users/revenge/code/psc-nanoid/src/packed.rs:199-211`
- **What:** The doc says "The caller must ensure the bytes represent a valid packed `Nanoid`", but nothing downstream relies on that for memory safety: `unpack` validates every extracted index against `A::VALID_SYMBOL_LIST.len()` (`packed.rs:259-264`) and returns `PackError::InvalidIndex`, and `as_bytes` just hands back a `[u8; B]`. There is no `unsafe` read of this data anywhere.
- **Why it matters:** An `unsafe fn` with no real invariant is worse than useless: it trains callers to write `unsafe {}` around something harmless, which is exactly the habit that later gets applied to something that is not. It also forces consumers into `unsafe` blocks in otherwise safe modules.
- **Fix:** Make it a safe `from_bytes`. If you want to keep the name, drop `unsafe` and keep a `# Note` saying the bytes are not validated until `unpack`. The one real caveat that *is* worth documenting is P-03's: the bytes must have been produced with the same `B`.
- **Effort:** S
- **Blast radius:** Removing `unsafe` from a function signature is backward-compatible for callers (their `unsafe {}` block just becomes an unused-unsafe warning). No internal caller found.

---

### [P-08] `PackedNanoid::default()` produces a real-looking ID

- **ID:** `psc-nanoid-full-08`
- **Severity:** Low
- **Category:** Design
- **Confidence:** High (reproduced)
- **Location:** `/Users/revenge/code/psc-nanoid/src/packed.rs:305-312`
- **What:** `Default` returns all-zero bytes. `PackedNanoid::<16, 12, Base62Alphabet>::default().unpack()` returns `Ok("AAAAAAAAAAAAAAAA")`, a syntactically valid, parseable, storable ID. Upstream deliberately declined to give `Nanoid` a `Default`: `lib.rs:221` carries `#[allow(clippy::new_without_default)]`, which is an explicit "yes, we know, and no". Commit e2b2b72 added `Default` to the packed half only, contradicting that decision.
- **Why it matters:** A `Default` on an identifier type means `#[derive(Default)]` on any struct containing one silently mints a fixed ID. In a WorkTable row that becomes a primary key collision on the second default-constructed row, surfacing as a confusing `AlreadyExists` far from the cause. `honey_id-types/src/types/id_entities.rs:18-22` writes its own `Default for AppPublicId` calling `Nanoid::new()`, which is the correct shape; the packed `Default` invites the wrong one.
- **Fix:** Remove the impl, or keep it but rename the concept: a `PackedNanoid::ZERO` associated const makes the "sentinel" use case explicit without hooking into `#[derive(Default)]`. Check first whether WorkTable/DataBucket require `Default` for their row types; `DataBucket/src/util/sized.rs` and `WorkTable/src/mem_stat/mod.rs` reference `PackedNanoid`, and if a `Default` bound is what drove e2b2b72, this needs a short design discussion rather than a removal.
- **Effort:** S (removal) / M (if a `Default` bound in WorkTable forced it)
- **Blast radius:** Potentially breaking for WorkTable and DataBucket. Confirm before removing.

---

### [P-09] `PACK_BITS` panics at const-eval for 0- or 1-symbol alphabets

- **ID:** `psc-nanoid-full-09`
- **Severity:** Low
- **Category:** Correctness
- **Confidence:** High (by inspection)
- **Location:** `/Users/revenge/code/psc-nanoid/src/packed.rs:80`
- **What:** `const PACK_BITS: usize = (SYMBOL_LIST.len() - 1).ilog2() as usize + 1;`. A custom alphabet with zero symbols underflows `len() - 1`; with one symbol it calls `0usize.ilog2()`, which panics. Both are const-eval failures, so they are compile errors, not runtime ones, but the message is a raw arithmetic-overflow diagnostic pointing into this crate rather than at the user's alphabet. `Alphabet` is a public trait users are explicitly invited to implement (`alphabet.rs:39-55`), and a 1-symbol alphabet is a plausible degenerate test case.

  Related: `new_with` (`lib.rs:249`) builds `Uniform::from(0..A::VALID_SYMBOL_LIST.len())`, which panics at runtime with "empty range" for an empty alphabet. The `# Panics` doc at `lib.rs:228-233` mentions only RNG failure and non-ASCII symbols.
- **Fix:** Add `assert!(SYMBOL_LIST.len() >= 2, "alphabet must have at least 2 symbols")` to `AlphabetExt::VALID_SYMBOL_LIST`'s const block (`alphabet.rs:90-93`), next to the existing `assert_all_ascii`. That gives one clear message at the single point where the alphabet is first validated.
- **Effort:** S
- **Blast radius:** Would break any consumer with a degenerate alphabet. None exist.

---

### [P-10] Generation cost: one RNG sample per character, one `Uniform` construction per ID

- **ID:** `psc-nanoid-full-10`
- **Severity:** Low
- **Category:** Performance
- **Confidence:** High (measured)
- **Location:** `/Users/revenge/code/psc-nanoid/src/lib.rs:243-266`
- **What:** `new_with` builds a `Uniform` distribution per call (line 249) and then calls `rng.sample(distr)` once per character (line 251): 21 rejection-sampled draws for a default ID, each pulling at least one `u32`/`u64` from ChaCha12 and doing a widening multiply. The output buffer is a stack `[MaybeUninit<u8>; N]`, so there is **zero heap allocation per ID**, which is the important part and is already right.

  Measured on this machine (release, `--profile release`, single thread, values are the mean of 2,000,000 iterations):

  | shape | throughput | ns/id |
  |---|---|---|
  | `Nanoid<21, Base64UrlAlphabet>` (default) | **6.70 M ids/s** | 149 |
  | `Nanoid<16, Base62Alphabet>` (family standard) | **8.86 M ids/s** | 113 |

- **Why it matters:** It does not, at current scale. Every consumer mints IDs one at a time on a request path; 113 ns is invisible next to a WorkTable insert, let alone a network round trip. I am recording this so nobody optimizes it speculatively.
- **Fix (only if a profile ever says so):** For power-of-two alphabets (Base64Url, Base32, Base16) `PACK_BITS` divides evenly and `Uniform` is pure overhead: one `rng.fill_bytes(&mut buf)` plus `buf[i] & (len - 1)` is unbiased by construction and gives roughly a 3-5x speedup. For non-power-of-two alphabets (Base62, Base58, Base36) keep the rejection loop but hoist it: fill a `u64` and consume 10 six-bit fields with a rejection retry only on the out-of-range ones, which is what upstream JS nanoid does. Do **not** replace the rejection with a modulo; that reintroduces the bias this crate currently does not have.
- **Effort:** M
- **Blast radius:** internal to `new_with`. The `test_new_uniformity` test (`lib.rs:563-589`) is the guard that any such change must keep passing.

---

## Things that are right, stated explicitly

Because the brief asks specifically about these, and a clean answer is a useful answer:

- **The RNG is a CSPRNG, seeded per thread.** `Nanoid::new()` → `rand::thread_rng()` → `ThreadRng`, which is a `ReseedingRng<ChaCha12Core, OsRng>`: seeded from the operating system on first use, automatically reseeded every 64 KiB of output and across `fork()`. `Cargo.toml:20` pins `rand = "0.8.5"`; `Cargo.lock` resolves 0.8.5, a fresh resolve gives 0.8.7. It is per-thread, not per-call, so there is no per-ID seeding cost and no seed-reuse hazard. **There is no fallback path anywhere in the crate to a weak or deterministic source.** The only way to get one is to call `new_with` explicitly with one (P-05).
- **No modulo bias.** The mapping at `lib.rs:249-252` goes through `rand::distributions::Uniform`, whose `UniformInt` uses Lemire's widening-multiply method with rejection. This is the one bug the brief flagged as the classic nanoid implementation error, and it is not present. There is even an empirical guard: `test_new_uniformity` (`lib.rs:563-589`) generates 100k-400k IDs per configuration and asserts that `(max_count - min_count) / expected_count < 0.05` across all symbols, for six different `(N, alphabet)` pairs. That test would catch a naive `% len` regression.
- **Entropy is adequate for the actual uses.**

  | configuration | entropy | where |
  |---|---|---|
  | `Nanoid<21, Base64UrlAlphabet>` (crate default) | 21 × 6 = **126.0 bits** | crate default, no internal consumer |
  | `Nanoid<16, Base62Alphabet>` (family standard) | 16 × log₂62 = **95.3 bits** | session IDs, app public IDs, user public IDs |

  The 16-character Base62 form is what `api.support.cafe/src/id_types.rs:8`, `honey_id-types/src/types/id_entities.rs:15` and `auth.honey.id-backend/src/config.rs` all use, including for capability-bearing values: `api.support.cafe/src/service/session.rs:47` mints session identifiers this way. 95.3 bits is well above NIST SP 800-63B's 64-bit floor for session identifiers and leaves ~63 bits of margin even after a birthday-bound argument over 2³² live IDs. **No consumer is using an ID short enough to be brute-forceable.** The packed 12-byte form stores those 95.3 bits in 96 bits, which is essentially optimal.
- **The SPSC-analogue question, "can a consumer get a weaker variant by accident":** the only sub-16-character usage I found anywhere is in psc-nanoid's own tests (`N = 6, 10, 12`).
- **`try_from_str`'s `unsafe`** (`lib.rs:287-296`) reinterprets a `&[u8]` as `&[u8; N]` after an explicit `s.len() == N` check. Sound, and the comment correctly cites the stdlib code it was copied from.
- **`new_with`'s `MaybeUninit` dance** (`lib.rs:244-260`) is the documented idiom, every element is written before the `ptr.read()`, and the two SAFETY comments are accurate.
- **`clippy.toml`** banning `std::assert_eq`/`assert_ne` in favour of `pretty_assertions` is a nice touch; note that `packed.rs:668,671,675,676` (the rkyv tests) violate it and produce 14 clippy warnings today, so `cargo clippy --all-targets --all-features -- -D warnings` is also currently failing, which is the second half of P-02's red CI.

## Appendix A: every `unsafe` in psc-nanoid

| # | Location | Operation | Invariant relied on | Enforced? |
|---|---|---|---|---|
| 1 | `lib.rs:247` | `MaybeUninit::uninit().assume_init()` on `[MaybeUninit<u8>; N]` | An array of `MaybeUninit` needs no initialization | **Yes.** The documented stdlib idiom; comment cites the reference. |
| 2 | `lib.rs:257-259` | `(&mut buf as *mut _ as *mut [u8; N]).read()` | All `N` elements were written by the loop above | **Yes.** The `for b in &mut buf` loop at 250-252 writes every element unconditionally. |
| 3 | `lib.rs:290` | `&*(s.as_ptr() as *const [u8; N])` | `s.len() == N`, alignment 1 | **Yes.** Length checked at line 287 on the branch that reaches it. |
| 4 | `lib.rs:344` | `str::from_utf8_unchecked(&self.inner)` | `inner` is all-ASCII | **No.** Upheld by `try_from_bytes` (315-328), `new_with` (via `assert_all_ascii`, `alphabet.rs:110`), serde (474-482) and `PackedNanoid::unpack` (`packed.rs:259-266`), but **violated by the rkyv `Deserialize` impl** at `lib.rs:372-377`. This is P-01. |
| 5 | `packed.rs:206` | `pub const unsafe fn from_bytes_unchecked` | Stated as "bytes represent a valid packed Nanoid" | **Vacuous.** No `unsafe` read depends on it; `unpack` validates. This is P-07. |
| 6 | `packed.rs:593` (test) | `&*archived.as_ptr().cast()` to `&[u8; 16]` | rkyv's output buffer is at least 16 bytes and 1-aligned | **Yes**, and it is asserted on the next line. Test-only. |

Six sites, five of them sound. That is a good ratio, and the fifth is a fork-local addition rather than upstream code.

---

# Part 2: `bounded-spsc-queue`

Version 0.4.0. A fork of `polyfractal/bounded-spsc-queue`, last upstream activity around 2018-2020, with three local commits: allocator-API cleanup (7d4bb9b), zero-sized-type handling (115692f), and `Debug` impls (b98a4c3). Nothing in `/Users/revenge/code` depends on it.

The overall verdict: this is careful 2015-era lock-free code that mostly holds up. The head/tail split, the shadow-index caching, the absolute (non-modulo) index scheme and the power-of-two masking are all textbook-correct, and the `Drop` correctly drains rather than leaking. The problems are one wrong memory ordering, one reference-validity slip, arithmetic that assumes 64-bit, and a total absence of the verification tooling that a crate like this needs.

## Findings

### [B-01] `try_push` loads `head` with `Relaxed`, breaking the release/acquire pair with the consumer

- **ID:** `bounded-spsc-queue-full-01`
- **Severity:** High
- **Category:** Correctness (concurrency)
- **Confidence:** High that the ordering is wrong under the Rust/C++ memory model; Medium that it manifests on today's compilers and hardware
- **Location:** `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:188` (the bug), `:117` (its unpaired counterpart)
- **What:** The queue has two cross-thread handoffs, and only one of them is correctly synchronized.

  **Producer → consumer (correct).** The producer writes the slot non-atomically (`store`, line 195) then publishes with `self.tail.store(..., Ordering::Release)` (line 197-198). The consumer acquires with `self.tail.load(Ordering::Acquire)` (line 109) before reading the slot (line 115). Release/Acquire pair, happens-before established, the value is visible. Correct.

  **Consumer → producer (broken).** The consumer reads the slot out non-atomically (`ptr::read`, line 115) then publishes the freed slot with `self.head.store(..., Ordering::Release)` (line 116-117). The producer reads that with:

  ```rust
  self.shadow_head.set(self.head.load(Ordering::Relaxed));   // line 188
  ```

  **Relaxed**, not Acquire. It then writes into the slot the consumer just vacated (line 195).

  With a `Relaxed` load there is no *synchronizes-with* edge, so there is no happens-before between the consumer's non-atomic read of slot `i` and the producer's non-atomic write of slot `i`. Two unordered non-atomic accesses to the same location, at least one a write, is the definition of a data race, which is UB. As a corollary the `Release` at line 117 is currently doing nothing useful: a release store with no matching acquire establishes no ordering with anyone.
- **Why it matters:** On x86-64 a `Relaxed` load compiles to a plain `mov`, which already has acquire semantics at the hardware level, so this is benign there today. On AArch64 the consumer's `stlr` for `head` does order its prior slot load, so the hardware is also probably fine in practice. The real exposure is the **compiler**: LLVM is entitled to sink a `Relaxed` load past a subsequent store, or to hoist the store above the branch, because it is allowed to assume no data race exists. That is exactly the class of bug that shows up as a one-in-a-billion corrupted element after an LLVM upgrade, in a primitive whose whole selling point is that it is faster than a channel. And it is precisely the finding that miri and loom exist to catch, which brings us to B-05.
- **Fix:** One word. Mechanical, no performance cost on x86 (identical codegen) and one `ldapr`/`dmb ishld` on AArch64:

  ```rust
  self.shadow_head.set(self.head.load(Ordering::Acquire));   // was Relaxed
  ```

  For completeness, the full audit of every atomic access in the file:

  | line | op | ordering | verdict |
  |---|---|---|---|
  | 106 | `head.load` in `try_pop` | Relaxed | **Correct.** Consumer reading its own index; no other thread writes it. |
  | 109 | `tail.load` in `try_pop` | Acquire | **Correct.** Pairs with the producer's Release at 197. This is what makes the slot's contents visible. |
  | 116-117 | `head.store` in `try_pop` | Release | Correct in intent, but **currently unpaired** because of line 188. Becomes meaningful once B-01 is fixed. |
  | 131 | `head.load` in `skip_n` | Relaxed | **Correct.** Same as 106. |
  | 133 | `tail.load` in `skip_n` | Acquire | **Correct.** Same as 109. Arguably stronger than needed, since `skip_n` never reads slot contents, but harmless and future-proof. |
  | 141-142 | `head.store` in `skip_n` | Release | Same as 116-117. |
  | 185 | `tail.load` in `try_push` | Relaxed | **Correct.** Producer reading its own index. |
  | 188 | `head.load` in `try_push` | **Relaxed** | **WRONG, must be Acquire.** This finding. |
  | 197-198 | `tail.store` in `try_push` | Release | **Correct.** Orders the slot write at 195 before publication. |
  | 453 | `tail.load`/`head.load` in `Producer::size` | Acquire/Acquire | Stronger than needed (`Relaxed` suffices for an advisory count) but not wrong. |
  | 577 | same in `Consumer::size` | Acquire/Acquire | Same. |
  | 56-57 | `head.load`/`tail.load` in `Buffer::fmt` | Relaxed | **Correct.** Debug output is inherently a torn snapshot; nothing depends on it. |

  So: eleven of twelve orderings are right, and the one that is wrong is the one that lets the producer stomp a slot the consumer may still be reading.
- **Effort:** S
- **Blast radius:** one line. Not an API change.

---

### [B-02] `store` materializes a `&mut T` pointing at uninitialized memory

- **ID:** `bounded-spsc-queue-full-02`
- **Severity:** Medium
- **Category:** Correctness (unsoundness)
- **Confidence:** High that it violates the reference-validity rules; Medium that any current compiler miscompiles it
- **Location:** `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:248-252`
- **What:**

  ```rust
  unsafe fn store(&self, pos: usize, v: T) {
      let end = self.buffer.offset((pos & (self.allocated_size - 1)) as isize);
      ptr::write(&mut *end, v);
  }
  ```

  `&mut *end` creates a mutable reference to the slot **before** anything has been written there. On the first pass through the ring that memory is whatever `alloc::alloc` returned, i.e. uninitialized. The Rust reference requires that a reference always point to a valid value of its type. For `T = u64` that is vacuous, but for any `T` with a validity invariant (`bool`, `char`, `NonNull<_>`, any `enum`, any `NonZero*`, and every struct containing one) an uninitialized bit pattern is not a valid `T`, and constructing the reference is UB regardless of the fact that you immediately overwrite it. It also asserts uniqueness and `noalias` over that slot at a moment when the intent is precisely that nothing valid lives there.
- **Why it matters:** It is UB on paper today and a live miscompilation risk as LLVM's `noundef`/`noalias` handling on `&mut` parameters gets more aggressive. It is also the first thing miri will flag when someone finally runs it, which will look like a scary result for what is a one-character fix. `ptr::write` takes a raw pointer for exactly this reason.
- **Fix:** Drop the reference round-trip. Mechanical:

  ```rust
  unsafe fn store(&self, pos: usize, v: T) {
      let end = self.buffer.add(pos & (self.allocated_size - 1));
      ptr::write(end, v);
  }
  ```

  Switching `.offset(x as isize)` to `.add(x)` also silences clippy's `ptr_offset_with_cast` at lines 237 and 249 and removes a latent sign issue on 32-bit for very large `allocated_size`.

  The mirror-image site, `load` at lines 236-239, is *not* the same bug: it returns `&T` to a slot that is genuinely initialized (guaranteed by the `tail` Acquire load). It has a lesser problem: the returned `&T` borrows `&self`, so its lifetime extends well past the window in which the slot is guaranteed to stay untouched. In the one caller (line 115) it is consumed immediately by `ptr::read`, so it is fine today, but returning `*const T` would make it impossible to get wrong.
- **Effort:** S
- **Blast radius:** two private helper functions.

---

### [B-03] Index arithmetic overflows on 32-bit targets, and it will happen in about 90 seconds

- **ID:** `bounded-spsc-queue-full-03`
- **Severity:** Medium (High if 32-bit is an actual target)
- **Category:** Correctness
- **Confidence:** High (by inspection)
- **Location:** `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:187`, `:189`, `:453`, `:471`, `:577`
- **What:** Indices are absolute and monotonic, advanced with `wrapping_add` (lines 117, 142, 198), which is right. But three consumers of those indices use **non-wrapping** arithmetic:

  ```rust
  if self.shadow_head.get() + self.capacity <= current_tail {      // 187 and 189
  pub fn size(&self) -> usize { tail.load(..) - head.load(..) }    // 453 and 577
  pub fn free_space(&self) -> usize { self.capacity() - self.size() }  // 471
  ```

  When `head` approaches `usize::MAX`, `shadow_head + capacity` overflows: debug builds panic with "attempt to add with overflow"; release builds wrap to a small number, which makes `small <= current_tail` unconditionally true, so **`try_push` reports "full" forever** and `Producer::push` (lines 215-223) spins for eternity. Symmetrically, once `tail` wraps past zero while `head` has not, `size()` underflows to a value near `usize::MAX` and `free_space()` follows it.

  `skip_n` at line 137 gets this right, using `self.shadow_tail.get().wrapping_sub(current_head)`, which makes the inconsistency within a 767-line file its own smell.
- **Why it matters:** On 64-bit this is unreachable: at the README's claimed 50 M pushes/second, 2⁶⁴ operations take about 11,700 years. On **32-bit** it is 2³² / 5×10⁷ ≈ **86 seconds** of sustained throughput. 32-bit is not hypothetical for this crate: commit 1b40d40 is literally "Fix cache line padding on 32-bit targets", and the `cacheline_pad!` macro (lines 14-18) is written to compute correctly for a 4-byte `usize`. An audio or DSP pipeline on armv7, the archetypal use for a bounded SPSC ring of `f32`, hits this inside two minutes and then either panics or hangs.
- **Fix:** Use wrapping distance everywhere, matching what `skip_n` already does. Mechanical:

  ```rust
  // try_push, replacing lines 187 and 189
  if current_tail.wrapping_sub(self.shadow_head.get()) >= self.capacity { ... }

  // size(), both copies
  pub fn size(&self) -> usize {
      self.buffer.tail.load(Ordering::Relaxed)
          .wrapping_sub(self.buffer.head.load(Ordering::Relaxed))
  }
  ```

  The invariant that makes this correct is `0 <= tail - head <= capacity <= allocated_size <= usize::MAX/2`, so the wrapping difference is always the true distance. Add a test that seeds `head`/`tail` near `usize::MAX` and pushes across the boundary; it needs a test-only constructor or `#[cfg(test)]` access to the fields.
- **Effort:** S for the fix, M including the wraparound test
- **Blast radius:** internal. `size()`'s observable behaviour does not change on 64-bit.

---

### [B-04] The cache-line padding does not actually prevent false sharing: `Buffer` is not aligned

- **ID:** `bounded-spsc-queue-full-04`
- **Severity:** Medium
- **Category:** Performance
- **Confidence:** High (measured)
- **Location:** `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:12-18` (the macro), `:25-52` (the struct), `:588-590` (the test that misses it)
- **What:** The struct is laid out in three deliberate 64-byte groups (read-only fields, then the consumer's `head`/`shadow_tail`, then the producer's `tail`/`shadow_head`), each padded out with `_paddingN: [usize; cacheline_pad!(k)]`. The grouping is exactly right. But the struct carries no alignment attribute, so:

  ```
  size_of::<Buffer<u64>>()  = 192   (3 × 64, as intended)
  align_of::<Buffer<u64>>() = 8     (align_of::<usize>, NOT 64)
  ```

  Both measured. And `Buffer` lives inside an `Arc` (line 332), so its data sits at `size_of::<ArcInner>` = 16 bytes into an allocation whose alignment the allocator picks as `max(align_of::<Buffer>, ...)` = 16 on most platforms. The three 64-byte groups therefore start at an arbitrary 8- or 16-byte offset within the real cache lines, and each one can straddle two of them.
- **Why it matters:** The entire point of 128 bytes of padding is to keep the producer's write to `tail` out of the consumer's cache line for `head`. If the groups straddle line boundaries, the producer and consumer can still ping-pong a shared line, and you have paid 128 bytes per queue for nothing. On the current macOS allocator the 16-byte offset happens to place `head` and `tail` in different 64-byte lines, so the benchmark numbers look fine, which is worse, because it hides the problem behind an allocator implementation detail. `test_buffer_size` (line 588) asserts the *size* is `3 * CACHELINE_LEN` and so passes while the alignment is wrong; the test that would have caught this was never written.

  Second, related issue: `CACHELINE_LEN = 64` is wrong on two of the platforms this is most likely to run on. Apple Silicon uses 128-byte cache lines, and x86-64's adjacent-cache-line prefetcher effectively pairs lines into 128-byte units, which is why `crossbeam-utils::CachePadded` uses 128 on both `x86_64` and `aarch64`.
- **Fix:**

  ```rust
  #[repr(C, align(128))]
  pub struct Buffer<T> { ... }
  ```

  and widen `CACHELINE_LEN` to 128 for `x86_64`/`aarch64` (keeping 64 elsewhere) via `#[cfg(target_arch = ...)]`, or simply delete the hand-rolled padding and wrap the two hot pairs in `crossbeam_utils::CachePadded`, which already encodes the per-architecture table and is maintained. Then extend `test_buffer_size` with `assert_eq!(align_of::<Buffer<()>>(), CACHELINE_LEN)` and an assertion that `(&b.tail as *const _ as usize) / CACHELINE_LEN != (&b.head as *const _ as usize) / CACHELINE_LEN`.

  Note the `align(128)` change makes `size_of::<Buffer<()>>()` a multiple of 128, so the existing `3 * CACHELINE_LEN` assertion has to be updated in the same commit.
- **Effort:** S (attribute) / M (with the arch table and the alignment tests)
- **Blast radius:** `Buffer`'s size and alignment change; `test_buffer_size` must be updated. Not a source-level API break. Worth benchmarking before and after, since the current numbers may be accidentally good.

---

### [B-05] No loom, no miri, no CI at all for a lock-free primitive

- **ID:** `bounded-spsc-queue-full-05`
- **Severity:** Medium
- **Category:** Maintainability
- **Confidence:** High
- **Location:** `/Users/revenge/code/bounded-spsc-queue/.travis.yml`; absence of `.github/workflows/`
- **What:** The only CI configuration is a `.travis.yml` targeting travis-ci.org, which has been shut down for years; the README badge at line 4 points at a dead endpoint. There is no GitHub Actions workflow, no miri invocation, no loom dependency, and no `Cargo.lock`. The concurrency testing consists of `test_threaded` (lines 699-713), which pushes 100,000 `usize` through a 500-slot queue between two real threads and checks they come out in order. That is a smoke test: it exercises one interleaving on one machine's memory model and will pass on x86 no matter how wrong the orderings are. It would not have caught B-01.

  Compare psc-nanoid in the same family, which has an eight-job workflow covering fmt, clippy with `-D warnings`, `cargo hack --feature-powerset`, MSRV, docs, lockfile freshness, README regeneration and automated publishing. The concurrency crate has less verification than the string-formatting crate.
- **Why it matters:** B-01 and B-02 are exactly the two bug classes that loom and miri respectively are built to find, and both have been sitting in this file since the original upstream. Every future change to the ordering logic is currently reviewed by eye only.
- **Fix:** Three steps, in this order.

  1. **miri**, which is nearly free and finds B-02 immediately:
     ```bash
     cargo +nightly miri test
     ```
     Expect it to flag `store`'s `&mut *end` on uninit memory, and possibly the `&T` lifetime in `load`. Note miri will *not* find B-01 in a single-threaded test; you need the threaded test under `MIRIFLAGS=-Zmiri-preemption-rate=...` or, better, loom.

  2. **loom**, which model-checks every interleaving permitted by the C11 memory model and is the tool that catches B-01. It needs a `cfg(loom)` shim swapping `std::sync::atomic` and `std::cell::Cell` for `loom::sync::atomic` and `loom::cell::Cell`, and a smaller queue so the state space stays tractable. Sketch:

     ```rust
     // tests/loom.rs, run with: RUSTFLAGS="--cfg loom" cargo test --test loom --release
     #![cfg(loom)]
     use loom::thread;

     #[test]
     fn spsc_two_slots_wraps_once() {
         // Capacity 2 forces the producer to reuse a slot the consumer has freed,
         // which is exactly the consumer -> producer handoff that B-01 breaks.
         loom::model(|| {
             let (p, c) = bounded_spsc_queue::make::<u32>(2);

             let prod = thread::spawn(move || {
                 for i in 0..3u32 {
                     p.push(i);          // the 3rd push must reuse slot 0
                 }
             });

             let mut got = Vec::new();
             while got.len() < 3 {
                 if let Some(v) = c.try_pop() {
                     got.push(v);
                 } else {
                     loom::thread::yield_now();
                 }
             }
             prod.join().unwrap();
             assert_eq!(got, vec![0, 1, 2]);   // FIFO order, no lost or duplicated element
         });
     }
     ```

     With `head` loaded `Relaxed` at line 188, loom reports the unsynchronized access on the reused slot; with `Acquire` it passes. Keep the model tiny (capacity 2, three pushes) or the state space explodes. A second model with a `T` that counts drops catches leak/double-free regressions in `Buffer::drop`.

  3. **A GitHub Actions workflow** mirroring psc-nanoid's, minus the crate-specific jobs, plus a `miri` job and a `loom` job. Copy `psc-nanoid/.github/workflows/ci.yaml` as the starting point since the conventions are already established in this family.
- **Effort:** M for miri plus the workflow, L for a well-shaped loom suite
- **Blast radius:** new dev-dependencies (`loom` behind `cfg(loom)`) and CI config. No library code changes beyond the `cfg(loom)` import shim.

---

### [B-06] `unsafe impl<T: Sync> Sync for Buffer<T>` has the wrong bound and is load-bearing for nothing

- **ID:** `bounded-spsc-queue-full-06`
- **Severity:** Medium
- **Category:** Correctness / API design
- **Confidence:** High (compile-verified)
- **Location:** `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:71`, and `:85-86` for the impls that actually matter
- **What:** The brief asked me to scrutinize this with the WorkTable precedent in mind (data behind a hand-written `unsafe impl Sync` whose read path took no lock). **The precedent does not repeat here**, and I want to be precise about why, because the impl still deserves to be changed.

  The impl asserts `Sync` for a type containing two `Cell<usize>` (`shadow_tail` line 43, `shadow_head` line 50), which are `!Sync` precisely because they permit non-atomic interior mutation. `try_pop` mutates `shadow_tail` through `&self` (line 109) and `try_push` mutates `shadow_head` through `&self` (line 188). If two threads could each hold a `&Buffer<T>` and call `try_pop`, that would be an unsynchronized read-modify-write on a plain `usize`, a data race, and `Buffer::try_pop`/`try_push` are `pub` safe methods on a `pub` type.

  **But the SPSC contract is genuinely enforced by the type system, not merely documented.** I verified this by compilation:

  - `Producer<T>` and `Consumer<T>` do not implement `Clone`, so you cannot make a second producer or a second consumer.
  - Neither is `Sync`. `Producer<T>` holds `Arc<Buffer<T>>`, and `Arc<X>: Sync` requires `X: Send + Sync`; `Buffer<T>` contains a `*mut T` and there is no `unsafe impl Send for Buffer`, so `Buffer<T>: !Send` and therefore `Producer<T>: !Sync`. Compiling `fn is_sync<T: Sync>(); is_sync::<Producer<u64>>();` fails with:

    ```
    error[E0277]: `*mut u64` cannot be sent between threads safely
       = note: required for `Arc<Buffer<u64>>` to implement `Sync`
    ```

    so `&Producer` cannot be shared across threads either.
  - The `buffer` field is private and there is no accessor, so no user can obtain a `&Buffer<T>` at all.

  The upshot: at most one thread can ever call `try_pop`, and at most one can ever call `try_push`. The impl at line 71 is **never actually required by anything**. The cross-thread capability comes entirely from the two `unsafe impl<T: Send> Send` at lines 85-86, whose `T: Send` bound is the correct one, since the queue *moves* `T` from one thread to another.

  Which is also why the bound on line 71 is wrong. `T: Sync` is the requirement for *sharing* a `&T`; this queue never shares a `&T` across threads, it transfers ownership. The correct bound for the operation being performed is `T: Send`. As written, `Buffer<Cell<u32>>` is not `Sync` even though a `Producer<Cell<u32>>`/`Consumer<Cell<u32>>` pair works fine and is `Send`, and conversely the impl advertises `Sync` for `T: Sync + !Send` types where transfer would be wrong (unreachable today only because line 85-86 demand `T: Send`).
- **Why it matters:** Not exploitable today. It is exploitable after any one of: adding `impl Clone for Producer`, adding `pub fn buffer(&self) -> &Buffer<T>`, adding `unsafe impl Send for Buffer`, or making `Producer` `Sync` "so it can go in an `Arc`". Each of those is a plausible one-line convenience PR, and the `unsafe impl` at line 71 is what makes any of them compile silently into a data race. The safety argument that protects it is currently written down nowhere.
- **Fix:** Three parts, all small.
  1. Change the bound to `unsafe impl<T: Send> Sync for Buffer<T> {}`, or delete the impl entirely after confirming nothing needs it (I believe nothing does).
  2. Write the safety comment above lines 71 and 85-86 spelling out the enforcement chain: exactly one `Producer` and one `Consumer` exist because neither is `Clone`; neither is `Sync` because `Buffer` is not `Send`; therefore `shadow_head` is producer-thread-local and `shadow_tail` is consumer-thread-local. Anyone loosening any of those three facts must revisit this.
  3. Consider making `Buffer` private (`pub(crate)`) and exposing only `Producer`/`Consumer`. `Buffer<T>` is publicly nameable today (I compiled `bounded_spsc_queue::Buffer<u64>` in a probe) with public safe `try_push`/`try_pop`/`push`/`pop`/`skip_n`, none of which any user can reach. That is a documented API that is simultaneously a trap and unusable.
- **Effort:** S
- **Blast radius:** Making `Buffer` private is a breaking change on paper (nobody can use it, but it is in the public namespace). The bound change is not.

---

### [B-07] Release hygiene: this fork cannot be published, and the manifest is upstream's

- **ID:** `bounded-spsc-queue-full-07`
- **Severity:** Medium
- **Category:** Maintainability / Docs
- **Confidence:** High
- **Location:** `/Users/revenge/code/bounded-spsc-queue/Cargo.toml`, `README.md`
- **What:** Unlike psc-nanoid, which was renamed on fork, this crate kept every scrap of upstream identity:

  | field | current value | problem |
  |---|---|---|
  | `name` | `bounded-spsc-queue` | Owned on crates.io by polyfractal. `cargo publish` will 403. |
  | `version` | `0.4.0` | Identical to the published upstream version, so even with owner rights it is unpublishable. |
  | `repository` / `homepage` | `github.com/polyfractal/bounded-spsc-queue` | Points at upstream, not `pathscale/`. `AGENTS.md:38` meanwhile tells agents to open PRs against `github.com/pathscale/bounded-spsc-queue`. |
  | `documentation` | `polyfractal.github.io/bounded-spsc-queue/...` | A GitHub Pages site last published by Travis, which is dead. |
  | `authors` | Zachary Tong | Fine to keep for attribution, but there is no maintainer entry. |
  | `edition` | **absent** | Defaults to **2015**. `cargo` warns: "`package.edition` is unspecified, defaulting to `2015` while the latest is `2024`". |
  | `rust-version` | absent | No MSRV. |
  | `keywords` | `["data-structures"]` | One keyword, and it is a category name. Four slots wasted. |
  | `categories` | **absent** | Invisible in category browsing. `concurrency` and `data-structures` are both valid slugs and both apply. |
  | `Cargo.lock` | absent | Fine for a library, but it means CI (once it exists) cannot pin. |

  Dependency health, from an actual build:
  - `time = "0.1.39"` resolves to **0.1.45**, which carries **RUSTSEC-2020-0071** (segfault in `localtime_r`). Dev-dependency only, used solely for `PreciseTime` in two `#[ignore]`d benchmark tests (`src/lib.rs:715-716`) that duplicate what `benches/benchmark.rs` already does with criterion. `PreciseTime` was removed in `time` 0.2, so this pins the crate to a vulnerable major forever.
  - `criterion = "0.2.3"` resolves to **0.2.11** (2018) and triggers `warning: the following packages contain code that will be rejected by a future version of Rust: criterion v0.2.11`.

  And the repo's own stated invariants are both failing. `AGENTS.md` says "Keep `cargo fmt` and `cargo clippy --all-targets` clean. Lint failures are part of the build here, not advisory." Measured today: `cargo fmt --check` produces a diff (starting at `src/lib.rs:234`), and `cargo clippy --all-targets` produces **26 warnings**.
- **Why it matters:** If the intent is to publish a pathscale-maintained fork, none of it works as-is. If the intent is to keep it as an internal vendored dependency, the metadata is actively misleading about where to file issues and where the docs live, and nothing depends on it anyway (see the note below).
- **Fix:** Decide the intent first, then either:
  - **Publish path:** rename to `psc-bounded-spsc-queue`, reset the version (1.0.0 or 0.5.0), repoint `repository`/`homepage` to `pathscale/`, set `documentation = "https://docs.rs/psc-bounded-spsc-queue"`, add `edition = "2021"`, `rust-version`, `categories = ["concurrency", "data-structures"]`, and real keywords. Drop the `time` dev-dep by deleting the two `#[ignore]`d benchmark tests in `src/lib.rs` (criterion already covers them), and bump criterion to 0.5.
  - **Vendor path:** say so in the README and AGENTS.md, and still fix `repository`, the dead Travis badge, and the `time` dev-dep.

  Either way, run `cargo fmt` and clear the clippy warnings in one commit so the AGENTS.md invariant becomes true rather than aspirational. Do it *before* any of the correctness fixes above, so those diffs are not buried in a formatting churn.
- **Effort:** S
- **Blast radius:** manifest and README. A rename is breaking for any future consumer; there are none today.

---

### [B-08] `make(0)` returns a queue that deadlocks silently

- **ID:** `bounded-spsc-queue-full-08`
- **Severity:** Low
- **Category:** Correctness
- **Confidence:** High (reproduced)
- **Location:** `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:329-355`
- **What:** `make(0)` succeeds. `capacity = 0`, `allocated_size = 0usize.next_power_of_two() = 1`, so a one-element buffer is allocated. Then `try_push`'s guard becomes `shadow_head(0) + 0 <= tail(0)`, which is true forever, so every `try_push` returns `Some(v)`. Confirmed: `make::<u64>(0)` gives `capacity=0, size=0, try_push(1) -> Some(1)`. `Producer::push` (lines 215-223) therefore spins on a hot loop forever, as does `Consumer::pop`.
- **Why it matters:** A zero capacity is almost always a computed value (`make(n / workers)`, `make(config.depth)`), and the failure mode is a pegged core with no error, no panic and no log. A hang is the most expensive class of bug to diagnose.
- **Fix:** `assert!(capacity > 0, "bounded_spsc_queue::make: capacity must be non-zero");` at the top of `make`. Document it under the existing `# Panics` section (lines 324-328), which currently only mentions OOM.
- **Effort:** S
- **Blast radius:** turns a hang into a panic. Technically behaviour-changing.

---

### [B-09] `skip_n` leaks by design with nothing in the type system to stop you

- **ID:** `bounded-spsc-queue-full-09`
- **Severity:** Low
- **Category:** API design
- **Confidence:** High
- **Location:** `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:121-144` and `:519-541`
- **What:** `skip_n` advances `head` without reading the slots, so the skipped values are never dropped; the producer later overwrites them with `ptr::write`, which does not drop the old value. The doc is honest about it ("*WARNING:* This will leak at most `n` values... intended to be used on buffers that contain non-`Drop` data, such as a `Buffer<f32>`"), but there is no `T: Copy` bound, no `#[must_use]`, and nothing else preventing `Consumer<String>::skip_n`. Two smaller points: the warning sits under a `# Safety` heading on a **safe** function, which is the wrong heading (a leak is not unsound, and using `# Safety` here dilutes the meaning of that heading elsewhere in the file); and `Buffer::skip_n` differs from `try_pop` in setting `shadow_tail` unconditionally rather than only on a cache miss, which is a small pessimization but not a bug.
- **Fix:** Either bound it (`impl<T: Copy> Consumer<T> { pub fn skip_n(...) }`), which makes the misuse a compile error and matches the documented intent, or keep it general and rename the heading to `# Warning` while adding `debug_assert!(!std::mem::needs_drop::<T>())`. The bound is the better answer; nothing in the repo calls `skip_n` with a `Drop` type.
- **Effort:** S
- **Blast radius:** adding `T: Copy` is breaking for any caller using it with a non-`Copy` type. None exist in this tree.

---

### [B-10] Spin loops have no `spin_loop()` hint and never yield

- **ID:** `bounded-spsc-queue-full-10`
- **Severity:** Low
- **Category:** Performance
- **Confidence:** High
- **Location:** `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:159-166` (`Buffer::pop`), `:215-223` (`Buffer::push`)
- **What:** Both blocking methods are bare `loop { match self.try_pop() { ... } }` with no `core::hint::spin_loop()` and no fallback to `std::thread::yield_now()`. The docs are explicit that the strategy is a spin-wait and tell you to use `try_pop`/`try_push` if you want something else, which is a legitimate design choice for a low-latency primitive.
- **Why it matters:** Two costs the docs do not mention. Without the `pause`/`yield` instruction the spinning core burns power, holds the pipeline full of speculative loads, and on SMT starves its sibling hyperthread of issue slots. And with no yield at all, on an oversubscribed system (more runnable threads than cores, or a single-core container) the spinner can hold its timeslice while the thread it is waiting on is descheduled, turning a microsecond wait into a full scheduler quantum.
- **Fix:** Add `core::hint::spin_loop()` inside the `None` arm; it is free on every architecture and strictly better. Optionally add a bounded backoff: spin with the hint for ~64 iterations, then `std::thread::yield_now()`. Keep `try_pop`/`try_push` untouched so callers who want pure non-blocking behaviour are unaffected.
- **Effort:** S
- **Blast radius:** two functions. Benchmark, since a yield changes the latency profile of the blocking path.

---

### [B-11] Every doctest is broken, and `doctest = false` hides it

- **ID:** `bounded-spsc-queue-full-11`
- **Severity:** Low
- **Category:** Docs
- **Confidence:** High
- **Location:** `/Users/revenge/code/bounded-spsc-queue/Cargo.toml` (`[lib] doctest = false`), `src/lib.rs:97-104`, `:155-158`, `:176-183`, `:211-214`, `:286-322`, `:387-394`, `:405-415`, `:425-433`, `:443-451`, `:483-490`, `:501-514`, `:529-538`, `:546-555`, `:564-575`
- **What:** `doctest = false` in the manifest means none of the ~14 examples are ever compiled, and essentially none of them would compile. Concretely: `src/lib.rs:99` uses `buffer.try_pop()` with no `buffer` in scope; `:293` calls `make(100)` with no import; `:411` reads `match producer.try push(123)`, a missing underscore, so it is not even valid syntax; `:554` and `:574` assert on `producer.capacity()` and `producer.size()` inside `Consumer`'s documentation, where the binding is `consumer`. Only two examples (`:504` and `:532`) bother with `use bounded_spsc_queue::*;`.
- **Why it matters:** Every one of these renders on docs.rs as the canonical usage example for a `pub fn`, and a reader copying `producer.try push(123)` gets a syntax error. The `doctest = false` line is what let the rot accumulate: it was presumably added because the examples did not compile, which inverts the tool's purpose.
- **Fix:** Fix the examples (add the `use`, add the `let (p, c) = make(...)` preamble, fix the two typos, fix the `producer`/`consumer` mixups) and delete `doctest = false`. There are only fourteen. Then the `readme`-style regression can never come back.
- **Effort:** M
- **Blast radius:** docs only, plus doctests joining the test suite.

---

### [B-12] `#[derive(Debug)]` on `Producer`/`Consumer` imposes a spurious `T: Debug` bound

- **ID:** `bounded-spsc-queue-full-12`
- **Severity:** Low
- **Category:** API design
- **Confidence:** High
- **Location:** `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:74`, `:80`
- **What:** Commit b98a4c3 added `#[derive(Debug)]` to both handles. The derive generates `impl<T: Debug> Debug for Producer<T>`, but the underlying `Buffer<T>`'s hand-written `Debug` (lines 54-69) prints only the pointer address, capacity, indices and shadow, and it never touches a `T` and correctly carries **no** `T` bound. So `Producer<NotDebug>` is needlessly not `Debug`, which is the classic derive-bound pitfall.
- **Fix:** Hand-write both, matching the style already used for `Buffer`:

  ```rust
  impl<T> fmt::Debug for Producer<T> {
      fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
          f.debug_struct("Producer").field("buffer", &*self.buffer).finish()
      }
  }
  ```
- **Effort:** S
- **Blast radius:** strictly widens the set of types for which `Debug` is available. Non-breaking.

---

## Appendix B: every `unsafe` in bounded-spsc-queue

Ten sites. Two have real problems (rows 5 and 7), one has the wrong bound (row 1), and one has a theoretical hole that is unreachable in practice (row 9).

| # | Location | Operation | Invariant relied on | Enforced? |
|---|---|---|---|---|
| 1 | `lib.rs:71` | `unsafe impl<T: Sync> Sync for Buffer<T>` | At most one thread touches `shadow_tail`, at most one touches `shadow_head`, despite both being `Cell<usize>` behind `&self` | **Yes, but by accident and with the wrong bound.** Enforced by `Producer`/`Consumer` being neither `Clone` nor `Sync` (compile-verified) and by `buffer` being a private field with no accessor. The bound should be `T: Send`, since the queue transfers ownership rather than sharing references. The impl is not required by anything. See B-06. |
| 2 | `lib.rs:85` | `unsafe impl<T: Send> Send for Consumer<T>` | `T` values cross the thread boundary; `Buffer::drop` (and therefore `T::drop`) may run on either thread | **Yes.** `T: Send` is exactly the right bound for both facts. |
| 3 | `lib.rs:86` | `unsafe impl<T: Send> Send for Producer<T>` | Same as row 2 | **Yes.** |
| 4 | `lib.rs:115` | `ptr::read(self.load(current_head))` in `try_pop` | Slot `current_head` holds an initialized `T`, and the producer will not overwrite it before `head` is published | **Initialization: yes**, guaranteed by the `tail.load(Acquire)` at line 109 pairing with the producer's `Release` at 197. **Non-overwrite: not formally**, because the producer's `head` load is `Relaxed` (line 188), so there is no happens-before edge in that direction. See B-01. |
| 5 | `lib.rs:194-196` | `self.store(current_tail, v)` in `try_push` | Slot `current_tail` is free: the consumer has finished reading it and published `head` | **No.** Same missing edge as row 4, from the other side. This is the write half of B-01, and it is the one that can corrupt data. |
| 6 | `lib.rs:236-239` | `unsafe fn load`: `&*buffer.offset(pos & (allocated_size - 1))` | `buffer` non-null and correctly aligned; `allocated_size` a power of two so the mask is a valid modulo; the slot is initialized | **Yes for the pointer.** `allocated_size = capacity.next_power_of_two()` (line 335) and the allocation is `allocated_size * size_of::<T>()` (line 360-361), so the masked index is always in bounds. **Partial for the lifetime:** the returned `&T` borrows `&self`, so its lifetime outlives the window in which the slot is guaranteed untouched. Sound only because the single caller consumes it immediately. Returning `*const T` would remove the hazard. |
| 7 | `lib.rs:248-252` | `unsafe fn store`: `ptr::write(&mut *end, v)` | Slot in bounds; and, implicitly, that `&mut *end` is a valid reference | **Bounds: yes**, same argument as row 6. **Reference validity: no.** On the first pass through the ring the slot is uninitialized, and constructing `&mut T` to uninitialized memory is UB for any `T` with a validity invariant. See B-02. |
| 8 | `lib.rs:265-273` | `alloc::dealloc` in `Buffer::drop` | Layout matches the one `allocate_buffer` used; ZSTs skipped since they were never allocated | **Yes.** Both sites compute `Layout::from_size_align(allocated_size * size_of::<T>(), align_of::<T>())`, and both gate on `size_of::<T>() > 0` (lines 265 and 366). The preceding `while let Some(_) = self.try_pop() {}` (line 263) correctly drains, so no leak and no double free on the normal path. **Caveat:** if a `T::drop` panics inside that loop, the unwind skips `dealloc` and the backing allocation leaks. A leak, not UB, and low priority. |
| 9 | `lib.rs:330` | calling `unsafe fn allocate_buffer(capacity)` | `capacity.next_power_of_two() * size_of::<T>()` does not overflow | **Yes for realistic inputs**, via `checked_mul(...).expect("capacity overflow")` at lines 360-362. **Theoretical hole:** for `capacity > 2^63`, `next_power_of_two` panics in debug and returns `0` in release; `0 * size_of::<T>()` then passes `checked_mul`, `size == 0` takes the ZST branch (line 366-370) and returns a dangling pointer, and `allocated_size - 1` in `load`/`store` becomes `usize::MAX`, masking to arbitrary offsets off a dangling pointer. Requires an 8-exabyte capacity request, so unreachable, but B-08's `assert!` could cheaply become `assert!(capacity > 0 && capacity <= usize::MAX / 2)`. |
| 10 | `lib.rs:358-377` | `unsafe fn allocate_buffer` body: `alloc::alloc` | `size > 0` before calling `alloc`; null result routed to `handle_alloc_error` | **Yes.** The ZST path returns `align_of::<T>() as *mut T`, a correctly-aligned dangling pointer, which is the right answer for a zero-sized `T` (commit 115692f). Clippy flags the idiom and suggests `std::ptr::dangling_mut::<T>()`, which is clearer but equivalent. |

## Classic SPSC bug checklist

The brief listed these specifically, so here is each one with a verdict rather than only the failures.

| bug class | verdict |
|---|---|
| Index wraparound | **Present on 32-bit.** Indices are absolute and advanced with `wrapping_add`, which is right, but `try_push`'s guard and `size()` use non-wrapping arithmetic. B-03. |
| Integer overflow in `make` | Guarded by `checked_mul` (line 361); theoretical hole above `2^63` capacity (Appendix row 9). |
| Full-versus-empty ambiguity | **Absent, and elegantly so.** Because indices are absolute rather than modulo, empty is `head == tail` and full is `tail - head == capacity`, with no shared representation. The power-of-two `allocated_size` is only ever used for the mask, so `capacity` can be any value ≤ `allocated_size`. This is the right design and it is implemented correctly. |
| ABA | **Not applicable.** There is no compare-and-swap anywhere in the file; indices are monotonic and each is written by exactly one thread. |
| Dropping elements on queue drop | **Correct.** `Buffer::drop` (lines 256-275) drains with `try_pop`, which moves each `T` into scope and drops it, then deallocates. No double free, no leak. Two caveats: values skipped by `skip_n` leak by design (B-09), and a panicking `T::drop` skips the deallocation (Appendix row 8). |
| `Send`/`Sync` too permissive for `T` | **`Send` is exactly right** (`T: Send` on both handles). **`Sync` on `Buffer` has the wrong bound** (`T: Sync` should be `T: Send`) but is unreachable. B-06. |
| Panic safety if `T::drop` panics | Leaks the backing allocation; does not corrupt. A second panic while unwinding aborts, which is standard. Low. |
| Panic safety in `try_push` | **Correct.** `v` is moved into `ptr::write` only after the capacity check; on the full path it is returned as `Some(v)`, so no value is ever lost or double-dropped. |
| SPSC contract enforceable or merely documented | **Enforceable, and enforced.** Not `Clone`, not `Sync`, private `buffer` field, compile-verified. This is a genuine strength and the opposite of the WorkTable precedent. Guard it with the safety comment recommended in B-06. |

## Performance notes

- **False sharing:** the layout intent is correct (three 64-byte groups: read-only, consumer-owned, producer-owned) but the alignment is not enforced. See B-04. This is the one performance finding worth acting on.
- **Atomics per operation:** `try_pop` performs one relaxed load of its own `head`, one relaxed load of the cached `shadow_tail` (a plain `Cell`, not an atomic), one release store of `head`, and an acquire load of `tail` **only on a cache miss**. `try_push` is symmetric. So the steady-state cost is one atomic store plus one plain load per operation, with the cross-thread acquire amortized over up to `capacity` operations. That shadow-index scheme is the main reason this beats a channel, and it is implemented correctly.
- **Batching:** the obvious unexploited win. `skip_n` already demonstrates that the shadow indices give you the available run length in one shot (`shadow_tail.wrapping_sub(current_head)`, line 137). A `try_pop_slice(&mut [T]) -> usize` and `try_push_slice(&[T]) -> usize` pair would use the same computation and then `ptr::copy_nonoverlapping` up to the ring wrap, amortizing the atomic store across the whole batch. For an `f32` audio buffer, the archetypal use, this is the difference between one atomic per sample and one atomic per block. Two functions, maybe 60 lines with the wrap handling, and it composes with the existing API rather than replacing it.
- **Spin strategy:** see B-10.
- **README claims:** the "~50m push operations per second" and "Sync Channel performs ~170k operations per second" figures were measured on a dual-core 1.7 GHz Macbook Air, and `std::sync::mpsc` was rewritten on top of crossbeam in 2022, so the comparison is stale in the direction that flatters this crate. The four PNGs are from the same era. I did not re-run the benchmarks (`criterion` 0.2.11 does not build cleanly on a modern toolchain); if the README's numbers are load-bearing for anyone's decision, they need re-measuring after the criterion bump in B-07.

## `no_std` suitability

Asked for both crates; the answer is different in each.

- **psc-nanoid: close, but not currently `no_std`.** `#![no_std]` is absent, `lib.rs:103` imports from `std`, `as_str` uses `std::str`, `Display`/`Debug` use `std::fmt`, and the `nanoid!` macro re-exports `std` at `lib.rs:536` and refers to `$crate::std::...` in its body. Almost all of that is `core`. The genuine blockers are three: `thiserror` 1.0 is std-only (2.0 supports `no_std`), `rand::thread_rng` needs `std`, and `From<Nanoid> for String` (`lib.rs:435-439`) needs `alloc`. A `no_std` build would be a real feature for embedded ID generation: gate `new()` behind a `std` feature, keep `new_with` (which is already generic over the RNG) always available, move `String`/serde behind `alloc`, and bump `thiserror` to 2.0. Not trivial, but the type is `[u8; N]` with no allocation, so the core of the crate is already `no_std`-shaped. Worth a decision rather than drift.
- **bounded-spsc-queue: not `no_std`, and only one thing genuinely blocks it.** It uses `std::alloc` (which is `alloc::alloc`), `std::cell`, `std::fmt`, `std::sync::atomic` and `std::sync::Arc` (which is `alloc::sync::Arc`). Every one of those is available in `core` + `alloc`. The `extern crate core;` at line 1 and the mixed `core::`/`std::` imports (lines 3-10) suggest someone started this conversion and stopped. Converting is close to mechanical: `#![no_std]`, `extern crate alloc;`, and repoint the five `std::` imports. The result would be usable in embedded and kernel contexts, which is where a bounded SPSC ring is most wanted. Low effort, real payoff.

---

# Cross-cutting recommendations

### 1. Fix the psc-nanoid rkyv path, then audit every other `unsafe`-adjacent deserialization in the family

**Move:** P-01, plus a sweep. **Why:** the pattern is "serde validates, rkyv does not", and rkyv was added late to several crates in this family for the WorkTable persistence layer. psc-nanoid is one instance; there may be others where a `rkyv::Deserialize` reconstructs a type whose safe API assumes a validated invariant. **Plan:** fix psc-nanoid, then `rg 'impl.*rkyv::Deserialize' ~/code` and check each hand-written impl (derived ones are fine, they recurse into the fields' impls) for a missing validation step, prioritizing types with an `unsafe` accessor. **What breaks:** adding a `D::Error: Source` bound is technically breaking; internally everything already uses `rancor::Error`.

### 2. Make psc-nanoid's CI green, then never let it go red again

**Move:** P-02 and P-04, plus the 14 clippy warnings in `packed.rs`'s rkyv tests. **Why:** a red pipeline means the `release` job can never run, so the crate that nine repos depend on has no working automated publish path, and every future regression lands silently. **Plan:** rename in the doctests, regenerate the README, fix the clippy violations, add `rust-version`. One afternoon. Then make the branch protection require the checks. **What breaks:** nothing; the fixes are all in comments and manifests.

### 3. Close the "wrong const parameter" class of bug at the type level, not the review level

**Move:** P-03's `const` assertions, and consider going further: make `packed_nanoid_type!` the *only* documented way to name a `PackedNanoid`, and mention it in the type's own rustdoc (currently the macro is documented 250 lines away at `packed.rs:362-388` and the type's doc at `packed.rs:98-102` just tells you the formula and trusts you). **Why:** six call sites across four repos each independently recompute `ceil(N * bits / 8)` in their heads, and the failure mode is silent ID corruption in release builds. This is the "missing invariant enforcement" pattern: something checked by convention in many places that could be unrepresentable. **Plan:** const assertions first (small, immediate), then a follow-up that revisits the type once `generic_const_exprs` stabilizes, per the existing TODO at `packed.rs:361`. **What breaks:** any consumer with a wrong `B` stops compiling, which is the point; audit the six sites first so the change is not a surprise.

### 4. Decide what bounded-spsc-queue is for, and act on the answer

**Move:** B-07. **Why:** it is currently in an unstable equilibrium: forked and given a pathscale AGENTS.md and CLAUDE.md, but with an upstream manifest, no CI, no consumers, and two real bugs. Every additional day of that costs review attention (this document, for instance) on a crate nobody uses. **Plan:** three options, pick one explicitly. (a) *Use it*: fix B-01 through B-05, rename, publish, and adopt it somewhere that currently uses a channel on a hot path. (b) *Vendor it*: fix B-01 and B-02, note in the README that it is an internal fork, skip the publishing work. (c) *Archive it*: the ecosystem now has `rtrb` and `crossbeam`'s `ArrayQueue`, both actively maintained, both loom-tested, both handling everything in this document. If nobody has a use case, (c) is the honest answer and frees the maintenance budget. **What breaks:** nothing today, since there are zero consumers.

### 5. Give the concurrency crate at least the verification the string crate has

**Move:** B-05, contingent on recommendation 4 not landing on "archive". **Why:** psc-nanoid has eight CI jobs including feature-powerset checks and MSRV verification; bounded-spsc-queue has a dead Travis file. The asymmetry is exactly backwards relative to the cost of a bug in each. **Plan:** copy psc-nanoid's workflow, add miri and loom jobs, write the loom model sketched in B-05. **What breaks:** the first miri run will fail on B-02, which is the point.

### 6. Replace `fastrand` with psc-nanoid in pays.online-backend

**Move:** cross-repo, and the reason the brief mentioned it. **Why:** `pays.online-backend/src/services/payment/util.rs:24` generates 7-character payment links with `fastrand::alphabetic()`. `fastrand` is a wyrand-based generator that its own documentation says is not suitable for security purposes, and 52⁷ ≈ 1.03×10¹² is about **40 bits**: brute-forceable, and worse, sequentially predictable from observed outputs since the generator's state is recoverable. The sibling review at `pays.online-backend/docs/reviews/2026-07-27-security-auth-money.md:139` covers the authorization half (`GetPaymentDetails` accepts either `id` or `link` with no ownership check, and no rate limiting is wired up), which is what turns weak links into an enumerable disclosure of payment records.

  **psc-nanoid is already in that repo's dependency tree**: `pays.online-backend/Cargo.toml:11` declares `psc-nanoid = { version = "3.1", features = ["serde", "packed", "rkyv"] }`. The fix is an import change.

  **Plan:** `Nanoid::<12, Base62Alphabet>::new()` gives 71.4 bits in 12 characters, or `Nanoid::<10, Base36LowercaseAlphabet>::new()` gives 51.7 bits in 10 lowercase-and-digits characters if the link has to stay hand-typeable. Either is at least 12 orders of magnitude better than the current 40 bits, and both come from a CSPRNG. Note that `get_next_link` currently recurses on `WorkTableError::AlreadyExists` to handle collisions (util.rs:34), which at 40 bits is a real code path and at 71 bits becomes dead defensive scaffolding you can simplify to an error. **What breaks:** link length and character set change, so any existing links must keep resolving; keep the old column and only generate new links the new way. Confirm with the pays.online reviewer before acting, since that repo is theirs.

---

# Extending the crates.io metadata review

`/Users/revenge/code/crates-io-metadata-review.md` covers `worktable` and `endpoint-libs` only, dated 2026-07-26. Nothing in it addresses either crate here, so this section adds rather than repeats. Where it is now stale:

- **Stale, item 9 ("the pre-release trap"):** the review says worktable's newest stable is 0.8.23 and the newest release is `0.9.0-beta0.2.3`, so `cargo add worktable` silently installs 0.8.x. Per this session's memory notes, **worktable 0.9.1 was released on 2026-07-27**, the day after that review. If so, the pre-release trap is resolved and section 1.4's suggested README note should not be added as written. The rest of section 1.4 (that `worktable_codegen` is published with no `repository` field and is an orphan page) is unaffected and still stands. **Verify against crates.io before acting on 1.4.**
- **Still accurate and worth reinforcing:** every crate this family publishes is missing `documentation` and `homepage`. That now holds for four out of four audited (worktable, endpoint-libs, psc-nanoid, bounded-spsc-queue), which makes it a family-wide default rather than an oversight in any one manifest. Worth a checklist in whatever release procedure they share.
- **Newly added by this review, in the same shape as that document's tables:**

  | # | Crate | Problem | Severity |
  |---|---|---|---|
  | 11 | psc-nanoid | `description = "Fork of nid crate"`, which describes provenance, not function | High |
  | 12 | psc-nanoid | No `rust-version`, which breaks two CI jobs that read it (`rustup toolchain install null`) | Medium |
  | 13 | psc-nanoid | `rkyv` feature undocumented in the crate docs, though it is the feature nine repos enable | Medium |
  | 14 | psc-nanoid | Keyword `"uuid"` is misleading; `"nano"` is redundant next to `"nanoid"` | Low |
  | 15 | psc-nanoid | No `documentation` / `homepage` (same family-wide pattern as items 6) | Medium |
  | 16 | bounded-spsc-queue | `name` and `version` collide with the published upstream crate; unpublishable as-is | High |
  | 17 | bounded-spsc-queue | `repository`, `homepage` and `documentation` all still point at `polyfractal`, contradicting AGENTS.md | Medium |
  | 18 | bounded-spsc-queue | No `edition` key, so it silently builds as edition 2015 | Medium |
  | 19 | bounded-spsc-queue | One keyword, no categories | Low |
  | 20 | bounded-spsc-queue | Dev-dep `time = "0.1"` carries RUSTSEC-2020-0071; `criterion = "0.2"` triggers a future-incompat warning | Low |

  Two differences worth noting against that review's conclusions. First, **psc-nanoid is the family's best-configured manifest so far**: it is the only one of the four with `keywords`, `categories` and a `[package.metadata.docs.rs]` section (`all-features = true` plus the `doc_auto_cfg` rustdoc arg). If a template is ever extracted for the family, start from this one. Second, the earlier review's framing of missing metadata as "pure upside, no risk" (its suggested-order item 3) holds for psc-nanoid but **not** for bounded-spsc-queue, where the name collision means metadata cleanup is blocked behind a rename decision.

---

# What I did not cover

- **I did not run the benchmarks in either crate.** bounded-spsc-queue's `criterion` 0.2.11 does not build cleanly on the current toolchain, and its two in-tree `#[ignore]`d benchmarks need the vulnerable `time` 0.1. The README's throughput numbers are therefore unverified by me; I only note that they are from 2015 hardware and predate the `std::sync::mpsc` rewrite. The psc-nanoid throughput figures in P-10 **are** mine, measured this session in release mode on this machine.
- **I did not run miri or loom.** Both are recommendations (B-05), and both would take longer than this review; B-01 and B-02 are derived from reading the memory model, not from a tool. I state my confidence separately for "the ordering is wrong on paper" (High) and "it miscompiles today" (Medium). A human should confirm with loom before treating B-01 as urgent versus merely correct-to-fix.
- **I did not audit each consumer's `PackedNanoid` `B` value.** I checked `honey_id-types` (`<16, 12>`, correct) and `api.support.cafe` (uses the macro, correct). The other four sites (`auth.honey.id-backend/src/db/util.rs`, `nofilter.io-backend`, `WorkTable/src/mem_stat/mod.rs`, `DataBucket/src/util/sized.rs`) I only located, not verified. Adding the const assertion from P-03 will check all of them mechanically, which is the better path anyway.
- **I did not evaluate whether removing `PackedNanoid: Default` (P-08) breaks WorkTable or DataBucket.** Both reference the type and both may require `Default` bounds on row fields. That determination belongs to whoever owns those repos.
- **I did not check crates.io live** for the current published versions of either crate, unlike the metadata review, which verified against the API on 2026-07-26. My claims about publishability (B-07) rest on the name and version in the manifest, not on a live query. Confirm before acting.
- **Consumer-side security** (how session IDs are transmitted, stored, compared, and expired in api.support.cafe; whether the payment-link authorization gap is fixed) is out of scope here and covered by sibling reviews of those repos. My entropy analysis says the IDs are strong enough; it says nothing about what happens to them afterwards.
- **`zeroize` correctness** beyond noting that the derive exists and the test at `lib.rs:846-862` calls it. I did not verify that the compiler cannot leave a copy of the ID on the stack, which is the usual caveat with `Zeroize` on a `Copy` type, and `Nanoid` is `Copy` (`lib.rs:382`), which means every use has potentially made copies the zeroize cannot reach. Worth a look if anyone is actually relying on the `zeroize` feature; nobody in this tree enables it.

---

# Quick-start for the follow-up agent

**Read in this order:**

1. `/Users/revenge/code/psc-nanoid/src/lib.rs:368-378` and `:342-345`: the Critical finding, and the whole of it fits on one screen.
2. `/Users/revenge/code/psc-nanoid/src/packed.rs:213-270`: `pack_impl` and `unpack_impl`, the two functions behind P-03. Note the `dst_idx < B` guards and line 256.
3. `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:105-200`: `try_pop` and `try_push` back to back. Every memory-ordering decision in the crate lives in these 95 lines; read them as a pair and the asymmetry at line 188 is visible.
4. `/Users/revenge/code/bounded-spsc-queue/src/lib.rs:25-86`: the struct layout, the padding macro, and the three `unsafe impl`s. Needed to follow B-04 and B-06.
5. `/Users/revenge/code/psc-nanoid/.github/workflows/ci.yaml`: the reference CI for this family, and the template for B-05.
6. `/Users/revenge/code/crates-io-metadata-review.md`: the prior metadata work this document extends.

**Commands to reproduce everything asserted here:**

```bash
# psc-nanoid: 34 failing doctests (P-02), 37 passing unit tests
cd /Users/revenge/code/psc-nanoid
cargo test --all-features            # fails at the doctest stage
cargo test --all-features --lib      # 37 passed
cargo clippy --all-targets --all-features   # 14 warnings, all disallowed-macro in packed.rs rkyv tests
cargo read-manifest | jq -r '.rust_version'   # -> null  (P-04)

# bounded-spsc-queue: passes, but see below
cd /Users/revenge/code/bounded-spsc-queue
cargo test                           # 8 passed, 2 ignored
cargo fmt --check                    # diff from src/lib.rs:234 (AGENTS.md invariant violated)
cargo clippy --all-targets           # 26 warnings (AGENTS.md invariant violated)
cargo +nightly miri test             # NOT YET RUN; expect B-02 to fire here
```

**To reproduce P-01 and P-03**, build a throwaway crate outside both repos with path dependencies on psc-nanoid (features `packed`, `rkyv`) and:

```rust
// P-01: deserializes without error, and .as_str() on the result is UB
let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&[0xFFu8; 16]).unwrap();
let id: Nanoid<16, Base62Alphabet> =
    rkyv::from_bytes::<Nanoid<16, Base62Alphabet>, rkyv::rancor::Error>(&bytes).unwrap();

// P-03: run once in debug (panics at packed.rs:256), once in release (returns a *different* ID)
let good: Nanoid<16, Base62Alphabet> = "abcdefghijklmnop".parse().unwrap();
let bad: PackedNanoid<16, 4, Base62Alphabet> = PackedNanoid::pack(&good).unwrap();
println!("{:?}", bad.unpack().map(|x| x.as_str().to_owned()));
```

**Surprising things about the layout and conventions:**

- **psc-nanoid's crate name is `psc_nanoid` but the entire source still says `nid`.** This is the single most confusing thing in the repo and the root of P-02. There is no `[lib] name` override; the rename was package-level only.
- **`README.md` is generated**, not authored. Edit `README.tpl` and the doc comment at the top of `src/lib.rs`, then run `cargo readme > README.md`. A CI job (`readme`) diffs it and fails if you forget. Note that `README.tpl` itself still contains ten links to `docs.rs/nid` and badges pointing at `ciffelia/nid`.
- **`docs/release_procedure.md` step 2 tells you to update `src/main.rs`.** There is no `src/main.rs`; it means the `//!` header in `src/lib.rs`. Stale since the crate was a binary, if it ever was.
- **`clippy.toml` bans `std::assert_eq!` and `std::assert_ne!`** in favour of `pretty_assertions`. New tests must import from `pretty_assertions` or clippy fails the build. The rkyv tests added in the fork violate this.
- **bounded-spsc-queue is edition 2015.** No `edition` key in `Cargo.toml`. That is why `extern crate core;` is at the top of `lib.rs` and why the import style looks archaic. Do not "clean up" the `extern crate` lines without adding `edition = "2021"` in the same commit.
- **bounded-spsc-queue has `doctest = false`.** Any example you add to a doc comment will never be compiled or run. Fix the existing fourteen and remove the line (B-11) before adding more.
- **Both repos carry a pathscale `AGENTS.md` + `CLAUDE.md` pair** stating that `cargo fmt` and `cargo clippy --all-targets` must be clean. That is true of neither repo today: psc-nanoid has 14 clippy warnings, bounded-spsc-queue has 26 plus an fmt diff. Land the formatting fix as its own commit so the real fixes are reviewable.
- **`.claude/settings.json` and `.claude/hooks/ask-before-risky-commands.sh` in both repos** gate pushes, `cargo publish`, `gh pr merge` and deploys behind a prompt. Expect to be asked.

## Nits

- `psc-nanoid/docs/release_procedure.md:2` references `src/main.rs`, which does not exist; it means `src/lib.rs`.
- `psc-nanoid/README.tpl:11-20` defines ten reference links, all pointing at `docs.rs/nid/...` instead of `docs.rs/psc-nanoid/...`.
- `psc-nanoid/README.tpl:36-46` badges point at `ciffelia/nid` CI, `crates.io/crates/nid` and `docs.rs/nid`.
- `psc-nanoid/src/lib.rs:19` tells users to add `nid = "3.0.0"`; the crate is `psc-nanoid` 3.1.1.
- `psc-nanoid/src/lib.rs:535-536`: `#[doc(hidden)] pub use std;` exists only so the `nanoid!` macro can write `$crate::std::...`. Correct and necessary, but it makes `psc_nanoid::std` a real public path; worth a one-line comment explaining why.
- `psc-nanoid/src/packed.rs:174`: the comment `// SAFETY: unpack_impl validates all characters` sits above an entirely safe struct literal. No `unsafe` here; the comment is a leftover.
- `psc-nanoid/src/packed.rs:132` and `:156`: both `# Errors` sections say the function returns `PackError::InvalidIndex`, but `pack` actually returns `PackError::InvalidCharacter` (see line 220). Copy-paste drift.
- `psc-nanoid/src/alphabet.rs:76`: `AlphabetExt` is `pub(crate)` but appears as a bound in the public `impl<A: Alphabet + AlphabetExt> AlphabetPackExt for A` (`packed.rs:79`). Compiles, but it means `AlphabetPackExt` is accidentally sealed. If that is intended, seal it deliberately and say so.
- `psc-nanoid/src/lib.rs:524-526`: the `InvalidLength` arm of the `nanoid!` macro is `unreachable!()`, which is correct (the length is derived from the same string) but reads as defensive scaffolding without the one-line explanation.
- `bounded-spsc-queue/src/lib.rs:1`: `extern crate core;` is an edition-2015 relic and redundant.
- `bounded-spsc-queue/src/lib.rs:10`: `use std::usize;` triggers clippy's "importing legacy numeric constants", and the import is unused.
- `bounded-spsc-queue/src/lib.rs:369`: `mem::align_of::<T>() as *mut T`: clippy suggests `std::ptr::dangling_mut::<T>()`, which is clearer for the same behaviour.
- `bounded-spsc-queue/src/lib.rs:396`, `:417`, `:435`, `:453`, `:492`, `:516`: `(*self.buffer)` appears at six sites where auto-deref does the job; clippy flags each.
- `bounded-spsc-queue/src/lib.rs:263`: `while let Some(_) = self.try_pop() {}`: clippy suggests `while self.try_pop().is_some() {}`.
- `bounded-spsc-queue/src/lib.rs:673`, `:682`, `:690`, `:694`: `assert!(false, "...")` in tests; clippy's "this assertion is always false". `panic!("...")` says the same thing without the warning.
- `bounded-spsc-queue/src/lib.rs:715-741`: two `#[ignore]`d hand-rolled throughput benchmarks that duplicate `benches/benchmark.rs` and are the sole reason for the `time = "0.1"` dev-dependency (RUSTSEC-2020-0071). Deleting them removes the advisory.
- `bounded-spsc-queue/src/lib.rs:39` and `:46`: `/// Consumer cacheline:` and `/// Producer cacheline:` are doc comments attached to the *next field* rather than section markers. They render as that field's documentation. Use `//` or move them.
- `bounded-spsc-queue/Consumer` has no `free_space()` even though `Producer` does (`lib.rs:470-472`); the asymmetry is probably deliberate but undocumented.
- `bounded-spsc-queue/README.md:4`: the CI badge points at `travis-ci.org`, which no longer exists, so it renders as a broken image.
- `bounded-spsc-queue/.travis.yml:14`: contains an encrypted `secure:` token for a service that has been shut down. Harmless, but delete the file along with the token.
