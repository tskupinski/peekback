# TODO

## Attach a file to a session explicitly

Add a CLI command that records a file as part of a session, for files no hook,
transcript or turn scan attributes to it: a file you ask the agent about, or
one outside the project that you want listed with the session.

```sh
peekback activity add FILE [--session ID | --pane %N]
```

- Resolve the session like `show` and `send`: explicit ID, tmux pane, then the
  agent's environment variables, so the agent can run it for you from inside
  the session.
- Store it as a normal event in the session's history with its own source,
  such as `explicit`, so views show it like any other file and label where it
  came from. A new `Source` variant changes the event schema in
  `session-activity`.
- Decide whether it also works for ended sessions, and whether a matching
  `remove` is needed to detach a file.
