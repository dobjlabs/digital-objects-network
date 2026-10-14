# Working on Bitcraft

The Tauri host is in `src-tauri`, the game frontend is in `ui`, the startup page is in `shell`, and packaging and smoke-test scripts are in `scripts`. Shared Digital Objects code lives in the repository's existing libraries, interfaces, and services.

The desktop has its own Cargo workspace. Root workspace commands exclude it, so run its checks explicitly using the commands in README.md. The repository's guidance to use release builds for tests also applies here.

Read `ui/CLAUDE.md` before changing game behaviour; it describes the UI's rule documentation and maintenance requirements.
