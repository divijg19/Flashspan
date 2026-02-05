# Ascent Flash (Flashspan)

Ascent Flash is a desktop flash-mental-math trainer. It shows a sequence of numbers at a configurable speed; you add/subtract them mentally and then enter the final sum.

## Features

- Fast, consistent flashing: timing is driven by the Rust backend (no JS timers)
- Configurable session settings: digits per number, flash duration, delay between numbers, count, optional negative numbers
- Audio cues: beep during flashes, applause/buzzer on validation (toggleable)
- Auto-repeat: optionally run multiple rounds with a countdown between sessions
- Theme + color schemes

## Install (from Release)

Download the latest files from the GitHub Releases page for this repo. The release artifacts are named:

- Windows: `Ascent.Flash-Windows_setup.exe`
- macOS: `Ascent.Flash-macOS.dmg`
- Linux (Debian/Ubuntu): `Ascent.Flash-Linux_x86_64.deb`
- Linux (Fedora/RHEL/openSUSE): `Ascent.Flash-Linux-x86_64.rpm`

### Windows

1. Download `Ascent.Flash-Windows_setup.exe`.
2. Run the installer and launch “Ascent Flash”.

### macOS

1. Download `Ascent.Flash-macOS.dmg`.
2. Open it and drag the app into Applications.
3. If macOS blocks the first launch, use System Settings → Privacy & Security to allow it.

### Linux (.deb)

```bash
sudo dpkg -i Ascent.Flash-Linux_x86_64.deb
sudo apt-get -f install
```

Then launch from your app menu.

### Linux (.rpm)

Fedora/RHEL:

```bash
sudo dnf install ./Ascent.Flash-Linux-x86_64.rpm
```

openSUSE:

```bash
sudo zypper install ./Ascent.Flash-Linux-x86_64.rpm
```

## Development

```bash
bun install
```

## Available Scripts

In the project directory:

### `bun run dev`

Runs the app in development mode.

### `bun run build`

Builds the frontend for production to the `dist` folder.
