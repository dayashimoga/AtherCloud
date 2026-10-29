# AETHERSTREAM TOOLCHAIN MITIGATION RECIPES

This guide provides mitigation recipes for optional and auxiliary binaries required for hardware video transcoding and remote repository automation.

---

## 1. FFmpeg & FFprobe (Adaptive Bitrate Video Pipeline)

FFmpeg is used by Stage 3 for HLS segmentation, keyframe extraction, and hardware-accelerated transcoding (NVENC/QSV).

### Windows Installation
- **Via Winget (Recommended):**
  ```powershell
  winget install Gyan.FFmpeg
  ```
- **Via Chocolatey:**
  ```powershell
  choco install ffmpeg -y
  ```
- **Via Scoop:**
  ```powershell
  scoop install ffmpeg
  ```

### macOS Installation
- **Via Homebrew:**
  ```bash
  brew install ffmpeg
  ```

### Linux Installation
- **Debian / Ubuntu:**
  ```bash
  sudo apt-get update && sudo apt-get install -y ffmpeg
  ```
- **Arch Linux:**
  ```bash
  sudo pacman -S ffmpeg
  ```
- **Fedora / RHEL:**
  ```bash
  sudo dnf install ffmpeg
  ```

### Verification
  ```bash
  ffmpeg -version
  ffprobe -version
  ```

---

## 2. GitHub CLI (gh)

Used for Stage 1 automated remote repository provisioning and branch policy enforcement.

### Windows Installation
- **Via Winget:**
  ```powershell
  winget install GitHub.cli
  ```
- **Via Chocolatey:**
  ```powershell
  choco install gh -y
  ```

### macOS Installation
  ```bash
  brew install gh
  ```

### Linux Installation
  ```bash
  sudo apt-get install gh
  ```

### Authentication
  ```bash
  gh auth login
  ```

---

## 3. Server Fallback Architecture (Zero-Dependency Mode)

When FFmpeg is absent on the host:
1. The AetherStream daemon seamlessly activates **Direct RFC 7233 Range-Request Streaming** (206 Partial Content), serving pristine native video containers directly without transcoding.
2. The HLS engine logs an informational advisory and falls back to browser-native container playback via the universal web client.
3. Zero panics occur during development, integration testing, or file streaming workflows.