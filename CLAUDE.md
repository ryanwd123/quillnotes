# Project Instructions

## project info
This Markdown editor is intended to be used as a note-taking app. We will be implementing features like a file tree, adding folders, persistence settings, preferences menu, etc.

A goal of this project is to keep the binary nice and small and the memory usage low, CPU usage low, fast startup, and I want it to open real fast and be configurable. 

I want to make the UI experience similar to VS Code, so try to use keyboard shortcuts that are similar to VS Code and navigation similar. 

We don not have to maintain a strict compliance to commonmark

## presistence
This app is still in the development stage and has not been deployed anywhere. So when we make updates that impact the persistence, we do not need to worry about how the prior persistence settings will be impacted. We can manually delete them. 



## Workflow
- Follow the existing architecture and coding conventions
- Do not add dependencies unless they are necessary.
- Do not modify unrelated files.


## Verification
- After each logical phase, run the appropriate tests.
- Run the full test suite before considering the task complete.
- Fix warnings or errors introduced by your changes.
- run release builds at project completion or before a release, not after every small change.
- Do not claim something works unless it has been tested when testing is
  reasonably possible.


## Git workflow
- After completing each change or task, commit it automatically. Don't ask first.
- Stage only the files related to that change. Never use `git add -A` or `git add .` if there are unrelated modifications.
- Write commit messages in imperative mood ("Add parser for X", not "Added..."), with a short summary line under 72 characters and a body explaining why if the change isn't obvious.
- Make small, focused commits: one logical change per commit.
- Work on `main`. The public repo is the `github` remote (github.com/ryanwd123/quillnotes).
- Push to `github` only when I ask. Do not add remotes or create branches unless I ask.
- Never amend, rebase, reset, or force anything without asking.
- Never commit secrets, .env files, or large data files.

### Implementation
- Prefer simple, maintainable solutions over clever abstractions.
- Reuse existing code where practical.
- Keep functions and modules focused.
- Add comments only where the reason for the code is not obvious.
