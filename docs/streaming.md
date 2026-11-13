# Streaming the game's files

Keep the game's files on the computer and play on the Android phone:
the files come over the network as the game asks for
them, and stay on the device for the next time. The game itself always
runs on the device; only its files travel.

## Share them from a computer

For this Android fork, use the Windows companion file server built by the
**GLUE5 file server** GitHub Actions workflow. It needs the WebAssembly root
containing both `data/` and `b/<build>/`, not the retail Legacy installation.
Do not build Rust on the owner's Inspiron. For example in PowerShell:

```powershell
& "C:\GTA5\GLUE5-Server-UI-ee4a47e\glue5-serve.exe" "C:\GTA5\Servidor\playgta5-offline\mirror\playgta5.com"
```

The historical desktop app can also share files, but it is not the supported
Android fork's companion workflow. The original alternatives were:

- open GLUE 5 and turn on **Settings › Game › Share on the network**; it
  shares while the app is open, or
- run `glue5-serve`, the same server without the app (`mise run serve
  [folder]` from the source; `-- --help` lists its options).

It listens on port 47100 and announces itself on the local network.

## Play from it

On the phone, select **Server** as the source on PLAY, then open **GLUE SERVER**.
The page
lists the computers on the network that share the game, whether each has
the right build and how far it is. For a computer elsewhere, type its
address (`host` or `host:port`).

The first start downloads what the game needs to start (about a minute
on a home network); the rest comes while you play, in the background. On the
same page, **Download the whole game** copies everything, so the device
plays without the computer.

## Connection problems on Windows

- Use `IP:47100` (for example `192.168.0.2:47100`), with both devices on the
  same reachable LAN. This is not an HTTP URL.
- Run only one server on that port. Error 10048 means another process is
  already using it, not that the game data is damaged. The revised CLI
  reserves the port before scanning/packing and reports this conflict early.
- Allow the **actual executable path** for inbound TCP 47100 on the local
  subnet. Discovery uses UDP 5353/mDNS; direct IP does not need discovery.
- Check for an enabled **Block** rule for that executable, especially after
  Windows' initial network prompt was rejected. A conflicting block can
  defeat a later allow rule. Review/disable only the matching server block,
  keeping the restricted LAN allow rule; do not disable the firewall globally.
- A successful connection from the PC to itself does not prove that the
  phone can reach it. Confirm the Android process log says `connected to`
  and then records pack transfer. No cache deletion or reinstall is required
  to fix a firewall timeout.

The GLUE SERVER page retains the typed address and has **Reconnect** before
gameplay, without deleting cached files. An explicit Connect releases the
text editor so the keyboard does not postpone the connection indefinitely.

## Apple TV

Historical upstream documentation below; not a supported target of this fork.

The Apple TV has no folder of its own that the system keeps: it always
streams, and keeps what it received in a cache the system may empty when
it needs the space.

## How it works

The game's engine already reads its files as it would from the web, and
announces the reads it is about to make. GLUE 5 answers each read from
the device's copy when it is there, as fast as a local folder; a missing
part is asked of the server, and the announcements become requests ahead
of time. One TCP connection carries them, the parts the game is waiting
for first, each checked on arrival. The copy grows as you play and serves
the next sessions. The details are in the
[design notes](design.md#streaming-the-games-files).
