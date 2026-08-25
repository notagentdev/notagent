Explore this project and write down what an agent needs to know to work in it.

Work through the repository first:

1. Find the key configuration files (`Cargo.toml`, `package.json`, `pyproject.toml`, and the like) and read what they declare.
2. Work out the technology stack, the build process, and how the thing actually runs.
3. Map how the code is organised: the top-level modules and what each of them owns.
4. Find the conventions this project follows — naming, testing, commits, release — by reading the code and the git history rather than assuming them.

Then write your findings to `AGENTS.md` in the project root. If that file already exists, read it first and carry forward everything still accurate: the result is one coherent, current file, not an appended section and not a replacement that throws away what was right.

`AGENTS.md` is read by coding agents, so write for a reader who knows nothing about this project and cannot ask questions. Every statement must come from something you actually read — no assumptions, no generalities that would fit any repository. Prefer a concrete command, path, or naming example over a description of one. Write it in the language the project's own comments and documentation use.

Sections worth having, where the project has something to say:

- What the project is and what it does
- Build, test, and run commands
- Code layout and module boundaries
- Code style and naming conventions
- Testing conventions and how to run the suite
- Commit and pull-request conventions
- Anything a newcomer would otherwise get wrong

Keep it dense. A short file that is entirely true beats a long one padded with the obvious.
