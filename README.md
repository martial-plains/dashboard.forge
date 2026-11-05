# forge-plugin-dashboard


## Install with use_package

Add this to `~/.config/forge/init.rhai`:

```rhai
use_package("dashboard", #{
  vc: "https://github.com/martial-plains/forge-plugin-dashboard",
  commands: ["dashboard-open"],
  grant: #{ environment: true },
});
```

The `environment` grant lets the dashboard find `HOME` for its default banner.
Forge asks before running the package build command the first time.

## Building

This plugin is built against the Forge plugin SDK (`forge-plugin-sdk`),
taken from the Forge repository named in `Cargo.toml` (change the `git =`
URL if Forge lives elsewhere).

    rustup target add wasm32-wasip2
    cargo build --release --target wasm32-wasip2

Copy the resulting `.wasm` to `~/.config/forge/plugins/`.

## License

GPL-3.0-or-later. See the license file in this directory.
