<p align="center">
  <img src="docs/images/hero.webp" alt="GLUE 5: GTA V on an iPhone, an Android phone, a MacBook Pro and an iPad" width="100%">
</p>

<p align="center">
  <b>GTA V, running natively on your iPhone, iPad, Mac, Android phone and PC.</b><br>
  A recompiler and runtime for the game's WebAssembly build: no emulator, no JIT, nothing rendered in the cloud.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/iOS-000000?style=for-the-badge&amp;logo=apple&amp;logoColor=white" alt="iOS">
  <img src="https://img.shields.io/badge/iPadOS-000000?style=for-the-badge&amp;logo=apple&amp;logoColor=white" alt="iPadOS">
  <img src="https://img.shields.io/badge/macOS-000000?style=for-the-badge&amp;logo=apple&amp;logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/tvOS-000000?style=for-the-badge&amp;logo=appletv&amp;logoColor=white" alt="tvOS">
  <img src="https://img.shields.io/badge/Android-3DDC84?style=for-the-badge&amp;logo=android&amp;logoColor=white" alt="Android">
  <img src="https://img.shields.io/badge/Windows-0078D6?style=for-the-badge&amp;logo=data%3Aimage%2Fsvg%2Bxml%3Bbase64%2CPHN2ZyB4bWxucz0naHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmcnIHZpZXdCb3g9JzAgMCAyNCAyNCc%2BPHBhdGggZmlsbD0nd2hpdGUnIGQ9J00wIDBoMTEuNHYxMS40SDB6TTEyLjYgMEgyNHYxMS40SDEyLjZ6TTAgMTIuNmgxMS40VjI0SDB6TTEyLjYgMTIuNkgyNFYyNEgxMi42eicvPjwvc3ZnPg%3D%3D" alt="Windows">
  <img src="https://img.shields.io/badge/Linux-FCC624?style=for-the-badge&amp;logo=linux&amp;logoColor=black" alt="Linux">
</p>

<p align="center">
  <a href="https://github.com/mempage4k/glue5/stargazers"><img src="https://img.shields.io/github/stars/mempage4k/glue5?style=for-the-badge&amp;logo=github&amp;color=yellow" alt="GitHub stars"></a>
  <a href="https://github.com/mempage4k/glue5/forks"><img src="https://img.shields.io/github/forks/mempage4k/glue5?style=for-the-badge&amp;logo=github" alt="GitHub forks"></a>
  <a href="https://github.com/mempage4k/glue5/commits/next"><img src="https://img.shields.io/github/last-commit/mempage4k/glue5/next?style=for-the-badge&amp;logo=github" alt="Last commit"></a>
  <img src="https://img.shields.io/badge/Rust-000000?style=for-the-badge&amp;logo=rust&amp;logoColor=white" alt="Rust">
</p>

> [!IMPORTANT]
> GLUE 5 contains **no game files**. It turns files you supply, from GTA V's
> **WebAssembly build** (`game.wasm`, its translated shaders and its prepared
> data), into an app for your device. Files from a regular PC or console
> installation are not enough. GLUE 5 is not affiliated with Rockstar Games
> or Take-Two Interactive.

## Highlights

- **Native on every device.** The game's code is compiled ahead of time
  for your device's CPU and drawn with Metal on Apple devices, Vulkan
  elsewhere. MetalFX and FSR 2 upscaling, frame generation on Apple
  silicon, frame pacing matched to the display.
- **Made for touch and controllers.** An on-screen controller that hides
  as soon as you pick up a gamepad or a keyboard, haptics, aim assist and
  the game's own button prompts for whatever you hold.
- **GTA Online, without servers.** Freemode sessions between GLUE 5
  players, found automatically on your network and over the Internet.
  Friends, invites and joins, end-to-end encrypted.
- **Your files, on every screen.** Keep the game on one computer and
  stream it to your phone, tablet or Apple TV, or copy it once and play
  offline.
- **Mods in Luau, with a Studio.** Write mods in your browser, with every
  one of the game's 6,000+ native functions typed and documented. Menus
  that work with touch and controllers. Coding agents can drive it too.
  Glue Trainer, a complete trainer, comes with it.

## Screenshots

<table>
  <tr>
    <td width="50%" align="center"><img src="docs/images/app-start.webp" alt="The start page on a MacBook Pro: story mode, GTA Online, sandbox and benchmark"><br><sub>The start page: story mode, GTA Online, sandbox, benchmark</sub></td>
    <td width="50%" align="center"><img src="docs/images/studio.webp" alt="GLUE 5 Studio on a tablet, editing a mod with its menu live from the game"><br><sub>GLUE 5 Studio: a mod, its menu live from the game</sub></td>
  </tr>
</table>

## Platforms

| Device | Status | Graphics | Notes |
|---|---|---|---|
| **iPhone, iPad** | ✅ Playable | Metal · MetalFX | iOS 16.4 or later. Tested on iPhone 16 Pro and iPhone 13 Pro. |
| **Mac** | ✅ Playable | Metal · MetalFX | macOS 14 or later, Apple silicon. |
| **Android** | 🧪 Early | Vulkan | Tested on Pixel 8a. |
| **Windows** | 🧪 Early | Vulkan | Builds; little tested. |
| **Linux** | 🧪 Early | Vulkan | AppImage, X11 and Wayland; little tested. |
| **Apple TV** | 🧪 Preview | Metal | Streams the game's files from a computer. Run in the simulator so far. |

## Get started

1. **Have your game files.** GTA V's WebAssembly build: `game.wasm`, its
   `shaders` folder and its `data` folder.
2. **Build the app.** The game's code ends up inside the app, so nobody
   can hand you a ready-made one: you build your own. The easiest way
   needs only a GitHub account: [build it on GitHub](docs/github-build.md),
   free, from your own `game.wasm`, which stays private. With a computer
   set up for development, [build it yourself](docs/building.md).
3. **Install it and add the files.** [Getting started](docs/getting-started.md)
   shows how on each device.

## Documentation

| | |
|---|---|
| [Getting started](docs/getting-started.md) | Install the app and put the game's files on each device. |
| [Playing](docs/playing.md) | Game modes, controls, settings, performance and troubleshooting. |
| [GTA Online and friends](docs/online.md) | Sessions, friends and invites without servers, and how they stay private. |
| [Streaming the game's files](docs/streaming.md) | Play from a computer's copy on your phone, tablet or Apple TV. |
| [Mods and GLUE 5 Studio](docs/mods.md) | Write mods in Luau, in your browser or with a coding agent. |
| [Build on GitHub](docs/github-build.md) | The app from your `game.wasm`, with no development setup. |
| [Building from source](docs/building.md) | Toolchain, tasks, platforms, options and the repository's layout. |
| [Design notes](docs/design.md) | How it works inside: the recompiler, the renderer, streaming, mods. |

## FAQ

<details>
<summary><b>Is the game included?</b></summary>

No. GLUE 5 is the recompiler and the runtime around the game. You supply
the game's files, from its WebAssembly build.
</details>

<details>
<summary><b>Why can't I just download the app?</b></summary>

The app is made from the game's own code (`game.wasm`), compiled for your
device. An app built from it contains the game, so it is never shared:
everyone builds their own, from their own files. GitHub can do it for you
in your account ([how](docs/github-build.md)).
</details>

<details>
<summary><b>Will it run on my phone?</b></summary>

The game needs a recent device: on iPhone, a 13 Pro or newer plays well;
devices with less than 8 GB of memory use a low-memory profile (smaller
pools, lighter graphics) by default. In the game's benchmark an iPhone 16
Pro averages about 42 fps at the default settings. Phones get hot after a
while, and then slower: see [Performance](docs/playing.md#performance).
</details>

<details>
<summary><b>Is GTA Online the real one?</b></summary>

It is the game's own freemode, played between GLUE 5 players. Nothing
connects to Rockstar's services: sessions are hosted by the players, and
characters are kept on the device. See [GTA Online and friends](docs/online.md).
</details>

<details>
<summary><b>Do I need a JIT, a jailbreak or developer mode?</b></summary>

No. The game is compiled before it is installed, so it runs like any other
app. On iPhone and iPad you install it as your own app (sideloading, with
your Apple ID).
</details>

## Contributing

Bug reports, fixes and ports are welcome: read
[CONTRIBUTING.md](CONTRIBUTING.md) first, and follow the
[code of conduct](CODE_OF_CONDUCT.md).

## License

GLUE 5 is free software: you can redistribute it and modify it under the
terms of the [GNU General Public License](LICENSE), version 3 or (at your
option) any later version.

The patches in [`patches/`](patches/README.md) change other projects and
are offered under each project's own license, so they can go upstream.
The fonts and touch icons in `crates/glue5/assets` keep their licenses
([OFL](crates/glue5/assets/ui/OFL.txt),
[icons](crates/glue5/assets/touch/README.md)). The game itself is not part
of GLUE 5 and is covered by none of this.

## Acknowledgements

GLUE 5 is built on [wasm2c](https://github.com/WebAssembly/wabt/tree/main/wasm2c),
[wgpu](https://wgpu.rs), [SDL](https://libsdl.org), [egui](https://github.com/emilk/egui),
[Luau](https://luau.org) with [mlua](https://github.com/mlua-rs/mlua),
[Leptos](https://leptos.dev), [CodeMirror](https://codemirror.net),
[snow](https://github.com/mcginty/snow), [mise](https://mise.jdx.dev) and
many other open-source projects: the app's Credits page lists them all,
with their licenses.

Grand Theft Auto and GTA are trademarks of Take-Two Interactive Software.
GLUE 5 is an independent project, not endorsed by or affiliated with
Rockstar Games or Take-Two Interactive.
