# Herdr integration for grok-pi

`grok-pi` includes an opt-in, headless Herdr lifecycle bridge. It needs no separate Pi integration install and is a silent no-op when the process is not running inside Herdr.

The bridge reports the native Pi session identity plus authoritative `working`, `blocked`, and `idle` lifecycle state through Herdr's local socket. It does not add another terminal UI and does not modify Pi or Herdr files in your home directory.

## Set up Herdr

1. Install Herdr:

   ```bash
   curl -fsSL https://herdr.dev/install.sh | sh
   ```

2. Start Herdr:

   ```bash
   herdr
   ```

3. Create or select a workspace and open a pane for the project.

4. Run `grok-pi` inside that pane:

   ```bash
   cd /path/to/project
   grok-pi
   ```

   When `HERDR_ENV=1` is already present, you are inside Herdr. Do not start a nested Herdr session.

5. Verify the active pane from another shell or Herdr command pane:

   ```bash
   herdr agent get "$HERDR_PANE_ID"
   ```

   The detected agent should be `pi`; while a turn is running Herdr should show `working`, and after the root interactive session settles it should show `idle`.

## Enable or disable it

Open **F2 → Agent → Pi Herdr integration**.

- `off` is the default.
- Set it to `on` to enable the built-in bridge when running inside Herdr.
- Restart the whole `grok-pi` process after changing it.

The equivalent config is:

```toml
[ui]
pi_herdr = true
```

Set it to `false`, or remove the key, to disable it. `grok-pi --no-extensions` also skips this and every other bundled bridge extension for that process.

## Interaction with Herdr's stock Pi integration

Herdr can install a managed stock-Pi extension with `herdr integration install pi`. `grok-pi` does not require it. When the built-in bridge is active, the host skips only the auto-discovered Herdr-managed Pi file so two extensions cannot compete for the same authoritative `herdr:pi` lifecycle source. Explicit user `--extension` arguments remain untouched.

No global integration is removed or rewritten, so stock `pi` can continue using Herdr's managed integration outside `grok-pi`.

## Troubleshooting

- Check Herdr and its integrations:

  ```bash
  herdr --version
  herdr integration status
  ```

- Confirm the pane environment contains `HERDR_ENV=1`, `HERDR_SOCKET_PATH`, and `HERDR_PANE_ID`.
- Restart `grok-pi` after changing the F2 setting.
- If all bundled extensions were disabled with `--no-extensions`, start without that flag.
- The bridge fails closed: if the local socket is missing or unavailable, `grok-pi` continues normally without lifecycle reporting.
