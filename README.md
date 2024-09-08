# `dircacher`
A simple utility to improve system performance by caching inodes, meant to be run on startup

## Installation

### Arch based systems
Run `arch-prepare.sh` to load source files into the `arch-pkg` directory\
`cd` into the `arch-pkg` directory\
`makepkg` to build the pacman package\
`pacman -U` to install to your system\
An example:
```bash
./arch-prepare.sh
cd arch-pkg
makepkg
sudo pacman -U dircacher-bin{VERSION_HERE}.tar.zst
```

### Other installations
Repeat arch installation steps and convert the generated arch package to a deb/rpm using a tool. I might add rpm/deb packages at another point

## Configuration 
A file called `/etc/systemd/system/dircacher.service` should have been created, modify the ExecStart to pass in any mountpoints you wish to cache on startup, and then enable dircacher.service on startup.\

For example on a btrfs system that splits home and root into two subvolumes:
```
ExecStart=dircacher / /home
```
