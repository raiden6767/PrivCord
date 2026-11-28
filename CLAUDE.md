# Working on this repo

## UI work
For any UI change (desktop React or Android Compose), take components and inspiration from these and use the design skills
(ui-ux-pro-max and related): React Bits (github.com/DavidHDev/react-bits; MIT + Commons Clause), 21st.dev/community/components,
uiverse.io, mobbin.com/discover/apps/ios/latest, magicui.design, ui.watermelon.sh, untitledui.com/react/components, jitter.video/templates/ui-elements, reactbits.dev/c/components. Some of them refuse automated fetches (403): use what loads, or
ask for pasted content. The desktop has no Tailwind, so effects are re-implemented in plain CSS (see
`apps/desktop/src/polish.css`); keep them cheap and honour `prefers-reduced-motion`. Check licences before copying code.

## Before every push
Run `scripts/precheck.sh` (add `--quick` to skip the Rust lints): formatting, the migration map, types and unit tests, the
checks that failed in CI several times this project for want of a local run. `docs/BUG_CLASSES.md` lists the classes of
bug fixed so far and what guards each; read it before adding a service, a message shape or an engine/screen value.

## CI minutes are limited
The budget is **2,000 GitHub Actions minutes a month** (Windows runners count double). Only run CI when it is needed.
- Every merge to `main` starts CI and the Test builds (Windows and Android installers) again. Merge in as few pushes as
  possible: batch related changes into one PR, and cancel runs a later merge makes redundant (the cancel tool of the
  GitHub MCP server works; `gh` is blocked here).
- Dependabot PRs cost a full CI run each when opened and again when merged: prefer grouped updates, merge several in one go,
  close the ones that fail (they come back as a new version).
- Test locally first (type-check, unit tests, builds, Playwright) and push once, not in small pieces: every push to a pull
  request restarts its CI.
- A change that touches only `site/` or root-level `.md` files does not run the app's CI (see `paths-ignore` in
  `.github/workflows/ci.yml`); the landing page is built and deployed by `pages.yml`. Build and test the site locally.
- Do not re-run failed jobs more than once, do not push empty commits to trigger CI, and merge `main` into a branch only when
  there is a conflict or it is needed to merge.
- Prefer draft pull requests while work is in progress; mark them ready when they are.
- Do not add heavy jobs (end-to-end, fuzz, emulator, installer builds) to run on every pull request without asking.

## Site and credits
The landing page (`site/`) does not show a credit for React Bits. Keep the licence notices in the copied component files and
`site/THIRD_PARTY.md`, because the licence requires them.
