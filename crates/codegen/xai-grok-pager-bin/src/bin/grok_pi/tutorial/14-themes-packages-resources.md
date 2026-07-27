# Themes, Packages & Resources

Pi Packages bundle extensions, Skills, Prompt Templates and themes from npm,
Git or local paths. Install and maintain them with Pi's CLI—for example
`pi install`, `pi remove`, `pi list` and `pi update`.

Inside grok-pi:

- `/theme` can select Pager themes or mapped Pi JSON themes such as `pi:<name>`;
  transparent dark and light themes preserve opaque code, diff and selection
  surfaces while leaving the main canvas to the terminal.
- `/pi-config` discovers Pi resources, previews README/package metadata and
  manages user or trusted-project enable/disable overrides.
- The same modal can filter, search, refresh and set source allow/block policy.
- Native feature conflict rules block known duplicate packages while an
  equivalent grok-pi bridge is enabled; explicit user allow still wins.

`/pi-config` does not install, remove or update packages. Use Pi's package CLI
for that, then `/reload` or restart when the changed resource requires it.
