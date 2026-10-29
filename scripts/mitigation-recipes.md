# AETHERSTREAM TOOLCHAIN MITIGATION RECIPES
If any required binary is absent, execute the corresponding recipe:

### 1. FFmpeg & FFprobe
- **Windows (Winget):** winget install Gyan.FFmpeg
- **Windows (Chocolatey):** choco install ffmpeg
- **macOS (Homebrew):** brew install ffmpeg
- **Ubuntu/Debian (APT):** sudo apt-get update && sudo apt-get install -y ffmpeg

### 2. GitHub CLI (gh)
- **Windows:** winget install GitHub.cli
- **macOS:** brew install gh
- **Linux:** sudo apt install gh
- **Authentication:** gh auth login
