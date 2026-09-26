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

## Viewer accessibility basics

From the viewer UI review (findings A1 and A2).

- Announce messages to VoiceOver: make the status message a live region
  (`role="status"`). Today only the picker note has one, so send results,
  toasts and questions are silent (WCAG 4.1.3).
- Show each message in one place: toasts for send results, the status line
  for everything else. Toasts now repeat their text in the status line.
- Add a visible `:focus-visible` ring, 2 px, to every input and button, in the
  theme's accent, which the theme contrast test already checks at 3:1. Inputs
  set `outline: 0` and buttons have no focus style (WCAG 2.4.7).
- Check focus by hand in the native window; the page tests can cover the
  attributes.

## Decision: what a clipboard send does with its comments

Parked until the current send flow has been used for a while. Today a paste
that falls back to the clipboard asks "Pasted? y marks sent · n keeps", which
puts the viewer in CONFIRM mode and blocks sending until answered. `Esc`
means "keep them" there but "leave" everywhere else, and outside tmux every
send falls back, so the question comes every time.

Proposal from the product review: a clipboard copy counts as sent. Toast
"Copied N comments · Cmd-V in the terminal · u brings them back", hide the
viewer, and let `u` restore the whole batch as pending. That removes the
question, CONFIRM mode and the send lock.

Settled while discussing it:

- Sending removes comments. They are a draft of one message to the agent; the
  terminal keeps what was sent. Keeping them would need a sent status or a
  manual clear, and pulls toward a review system (threads, resolved states).
- If checking a revision against the comments you sent turns out to matter,
  the smallest answer is the last sent batch, read-only, until the next send.
- Changed text stays tracked so the prompt never points at lines that now hold
  something else, but the viewer can show it in fewer places: keep the `]c`
  label and the note in the sent text, and drop the status-line count and the
  sidebar and picker labels.

## Decision: one document list instead of Current session and Scratchpad

Parked until the picker has been used for a while. Today Scratchpad is its
own scope, and a file the agent wrote there from the shell mid-turn shows
only under it, so you have to know to look there. Proposal from the product
review: Current session lists the session's recorded files and the live
scratchpad listing together, deduplicated and newest first, and the hotkey's
"newest document" considers both. `Tab`/`Shift-Tab` then cycle only to
Bookmarks. This touches how the newest document is chosen, so it is the
larger change.

## Needs testing: hide the viewer after a send

Try by hand before deciding. After a successful paste the next action is
Enter or typing in the terminal, so the viewer could hide itself and return
focus, saving an `Esc` on every send. Open questions:

- Does it feel right for `S` (send all comments)? Likely yes.
- For `s` (send a selection), do you often send several selections in a row?
  If so, `s` should keep the viewer open.
- A clipboard fallback is part of the parked clipboard decision above.
