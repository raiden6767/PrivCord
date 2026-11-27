# Contributing to PrivCord

Thanks for helping. PrivCord is a private, end-to-end encrypted, peer-to-peer messenger: a Rust engine shared by a Windows
app (Tauri + React) and an Android app (Kotlin + Compose).

## Where to start

- **Security problems:** report privately (see [SECURITY.md](SECURITY.md)), not in a public issue.
- **Bugs:** open an issue with what you expected, what happened, your platform and app version, and steps to reproduce. Do
  not paste real conversations, keys or recovery phrases.
- **Ideas and bigger changes:** open an issue first and say what you would like to change, so the work is not wasted. Good
  first areas: documentation, tests, translations of wording, small UI fixes, the self-hosted relay (`relay/`).
- **What not to change without a conversation:** the wire formats and cryptography (`crates/privcord-crypto`,
  `crates/privcord-domain`). Those need test vectors, a fuzz target for any new parser, and a read of
  [docs/BUG_CLASSES.md](docs/BUG_CLASSES.md) and [docs/CRYPTO_BASELINE.md](docs/CRYPTO_BASELINE.md).

## Set up

You need Node.js 20+ and a recent stable Rust (the version is pinned in `rust-toolchain.toml`).

```bash
git clone <your fork> && cd PrivCord
npm install
npm run dev:desktop      # the UI in a browser at http://127.0.0.1:1420 (a development store, not encrypted at rest)
npm run tauri:dev        # the real Windows/desktop app (needs Rust)
```

The Android app (`apps/android`) needs the Android SDK and NDK; the Kotlin code only compiles in CI or in Android Studio.
The build steps are in the [README](README.md#for-developers) and [docs/rust/ANDROID.md](docs/rust/ANDROID.md). If you cannot
build it, say so in your pull request and keep Android changes small.

## Before you open a pull request

Run the project's checks in one go:

```bash
scripts/precheck.sh          # formatting, the migration map, types, unit tests and the Rust lints
scripts/precheck.sh --quick  # the same without the Rust lints
```

It runs, in order: `cargo fmt --all --check`, the migration-map check, the TypeScript type check, the desktop unit tests, the
domain and crypto Rust tests and (without `--quick`) `cargo clippy --workspace --all-targets --locked -- -D warnings` (CI also adds `--all-features`).
For the larger test suites on their own:

```bash
cargo test -p privcord-engine                      # add PRIVCORD_SKIP_REAL_NOSTR=1 if you have no network
cd relay && npm test                               # the self-hosted relay
npm run e2e --workspace @duochat/desktop           # real browsers, relay, Nostr, TURN (needs Playwright)
```

Protocol or crypto changes need tests. A new message shape needs a case in
`crates/privcord-domain/tests/blob_wire_vectors.rs`.

## The pull request workflow

1. Fork, then branch from `main`. One topic per branch.
2. Keep the change small and focused. A pull request that mixes a refactor, a fix and a feature is hard to review.
3. Test locally first (above). **Push once, not in small pieces.**
4. Open the pull request as a **draft** while it is in progress; mark it ready when it is. Say what changed, why, how you
   tested it, and what you could not test (for example "not run on a real Windows machine").
5. Describe user-visible changes in plain words, and link the issue it answers.
6. A maintainer reviews and merges. Pull requests from forks need a maintainer to approve the CI run.

### CI is limited

The project has a small monthly budget of GitHub Actions minutes, and every push to a pull request restarts its CI. That
is why the checks above are run locally. Please do not push empty commits to trigger CI, and do not re-run failed jobs
more than once. A change that only touches `site/` or root-level `.md` files does not run the app's CI.

## Style

- Rust: no `unwrap`/`expect` or indexing in production code (clippy enforces it); secrets use `Zeroizing` and have a
  redacting `Debug`.
- Words shown to people are plain and short; no hard-coded sizes in the UI (it must adapt to the phone's screen).
- Do not add third-party services, analytics or crash reporting. See [METADATA_AUDIT.md](METADATA_AUDIT.md) for what the
  app is careful not to reveal.
- Keep licence notices in copied files (see [CREDITS.md](CREDITS.md) and `site/THIRD_PARTY.md`).

## License

PrivCord is source-available under the PC-NC license (see [LICENSE](LICENSE), [NOTICE](NOTICE) and
[COMMERCIAL_LICENSING.md](COMMERCIAL_LICENSING.md)). By contributing you agree your contribution may be distributed under
it, and that attribution to the project ("PrivCord by anandkrishna1900") is kept.
