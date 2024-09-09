# dircacher
A simple utility to improve system performance by caching inodes, meant to be run on startup.

The basic idea behind `dircacher` is that a program will first have to check if a file exists before it 
can be opened (whether this is handled by the kernel in a TOCTOU safe manner is irrelevant). And that there 
exist programs that will not actually open any files but instead read their metadata.

All that `dircacher` does is read every inode on given subfolders, without traversing symlinks or separate filesystems, in a 
multithreaded fashion. On a linux system with enough spare RAM, this will store the filesystem tree including metadata in the "fscache", 
leading to decreased latency when a user is interacting with any part of the filesystem.

In my experience, this has made my computer noticeably snappier after a reboot and takes very little time to do its work;
3 HDD RAID5 array with 1M inodes + 256GB SATA SSD with 2M inodes cached completely in 1m45sec.\
Others are welcome to share their own startup times and experience with `dircacher`, perhaps a table could be made

## Installation

### Arch based systems
1. Run `arch-prepare.sh` to load source files into the `arch-pkg` directory\
1. `cd` into the `arch-pkg` directory\
1. `makepkg` to build the pacman package\
1. `pacman -U` to install to your system\
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
A file called `/etc/systemd/system/dircacher.service` should have been created, modify the ExecStart to pass in any mountpoints you wish to cache on startup, and then enable dircacher.service on startup.

For example on a btrfs system that splits home and root into two subvolumes:
```
ExecStart=dircacher / /home
```
