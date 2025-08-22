FluxPM - The Flexible Linux Package Manager

FluxPM is a modern, fast, and flexible binary package manager written in Rust, designed specifically for bootstrapping and managing custom Linux From Scratch (LFS) and LFS-like systems. It combines the speed of a binary package manager with a unique hybrid installation model, making it a powerful tool for building and maintaining a clean and stable custom distribution.
✨ Unique Features

FluxPM was built with a few core principles in mind, making it different from traditional package managers.

    Hybrid Installation Model: Not everything on a system is equal. FluxPM treats core system packages and user-facing applications differently:

        System Packages: Core components (libc, coreutils, drivers) are installed traditionally into the root filesystem (/usr/lib, /bin, etc.) for a stable, conventional base.

        Sandboxed Apps: User applications (browsers, games, etc.) are installed into isolated directories in /flux/apps. This prevents them from cluttering the system and allows for perfectly clean, simple removal.

    Init-System Agnostic: FluxPM is not tied to any specific init system. Whether you choose Dinit, s6, runit, or systemd, packages can be built with simple post-install scripts to integrate with your chosen init, giving you complete freedom.

    Bootstrapper & System Manager: FluxPM serves two roles. Use the --root flag to safely build your entire LFS system from a host OS. Once you boot into your new system, the same binary works as its native package manager.

    Fast and Secure: Built in Rust with a fully asynchronous, parallel backend, FluxPM is designed for speed. All packages are verified with SHA256 checksums to ensure integrity.

🚀 Getting Started
Prerequisites

    Rust and Cargo (for building FluxPM)

    A C compiler and basic build tools (for building packages)

    zstd for package compression

Installation

    Clone the repository:

    git clone https://github.com/ElyasSindi2020/FluxPM.git
    cd FluxPM

    Build the release binary:

    cargo build --release

    Install the binary to your system's path:

    sudo cp target/release/FluxPM /usr/local/bin/flux

Configuration

Before using FluxPM, create a flux.conf file in the same directory (or in /etc/flux/ on a finished system). This file points to your repository.

flux.conf:

# The URL for the main package repository index.
repository_url: "[http://your-repo.com/packages.yaml](http://your-repo.com/packages.yaml)"

# System hooks (optional)
hooks:
  "linux-*": "/usr/local/bin/flux-hooks/update-bootloader.sh"

💻 Usage

FluxPM is simple to use. Here are the main commands:

    Update the repository cache:

    flux update

    Install a package into a target root (for bootstrapping):

    flux --root /mnt/lfs install zsh

    Install a package on the running system:

    flux install coreutils

    List all installed packages:

    flux list

    Remove a package:

    flux remove hello

    Remove orphaned dependencies:

    flux autoremove

📦 Building Packages

FluxPM uses pre-built binary packages. A repository is simply a web server hosting the package archives (.tar.zst) and a packages.yaml index file.

See the build-scripts directory for examples on how to compile and package software for a FluxPM repository.
🤝 Contributing

Contributions are welcome! If you have ideas for new features, bug fixes, or improvements, please open an issue or submit a pull request.
📜 License

This project is licensed under the MIT License. See the LICENSE file for details.
