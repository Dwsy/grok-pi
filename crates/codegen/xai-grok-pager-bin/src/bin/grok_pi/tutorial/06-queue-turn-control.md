# Queue & Turn Control

You can keep writing while Pi is already working.

- Press `Enter` during a running turn to create a **follow-up** for later.
- Promote a queued row with **send now** to steer the current turn without
  cancelling it.
- `/queue` opens the pending prompt list.
- Pager-owned rows can be edited, reordered, removed, cleared or promoted before
  dispatch while preserving stable ids and versions.
- Messages that bypass the grok-pi interception layer and live only inside Pi's
  external queue are shown read-only because stock RPC cannot atomically edit
  them.

Cancellation clears grok-pi-owned pending work and asks Pi to abort, then waits
for Pi to become idle before dispatching again. Steering remains part of the
current turn; follow-ups become later turns.
