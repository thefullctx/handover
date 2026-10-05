# CLI

The CLI is a thin client of the running daemon — it contains no application logic of its own.

```bash
handover status                 # daemon status + agent availability
handover agents                 # list configured agents
handover actions                # list available actions
handover send "Fix this error"  # send text
handover send ./error.log       # send a file
handover send ./screenshot.png  # send an image
cat error.log | handover        # pipe stdin (no subcommand = send)
```

Options for `send`:

```
--agent <id>        Agent to use (default: your per-action preference, else the default agent)
--action <id>       Action to use (default: ask)
--session <id>      Resume into this live session (overrides pinned/freshest)
--print-prompt      Print the rendered prompt without sending anything
```

Session commands:

```bash
handover sessions              # live sessions, freshest first, with activity/blocked state
handover attach <agent> [id]   # pin a session (run inside it; --tty records the approval target)
handover attach --unpin <agent># clear the pin (freshest-first resumes)
handover approve <agent> [id]  # approve a blocked session (--deny to deny)
```

← [Back to the README](../README.md)
