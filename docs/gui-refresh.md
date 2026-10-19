# Gui redisain
The gui is getting filled with so mane new features and it is starting to be unsuable.
So this file propeses solution to this proble by creating a redisign guid on how to refres it.

## Tabs
Now the gui is seperated into 7 config tabs and 3 others we will channge this seperation so it makes more sance now and it the future
- System
- Disk
- Packages
- Users
- Build and drivers
This will be all tabs, the edit as toml and flash/create iso will move to top bar where they will live only as button.
The tomel one would opne tab like interface where you can edit the toml.
The flash/create iso one will open tab like interface for flashing the iso ontu stic/floppy/...
The autoload button will also live there and it will give you a pop-up that ask you what you want to autolaod (no more reinstall preset)

## Generalr rules
We will stay with gtk styled app, and will more hevily use the command pallet.
There should be simple names not that pacman packages are imported and aur packages autoloaded on the same page (differnt names for the same thing)
Also some things are not very usable like scrolable list of text of the pacman packages, or the long list of overlays and aur packages.
For the last point i have a solution, put the list like in aur packages or the system overlay list into a smaller window like the pacmna packages and add a search bar, so it would be scrollabe small and searchyble. So better user expirienc.

## Tabs - concretly
this section is focused on what goes in each tab.

### System
There goes the system-wide config (not bootloader)
Like system overlay, language, keyboard, services (the system vide, not user ones) and post install scripts
There will also be the login manager config

### Disk
This tab will be focused on work with disk
There will go the bootloader, file system, eares disk or confirm serial, boot size and in future full partionig tool so we can enable dualboot with more linuxes.
In future we will support partions with btrfs for home, logging and stuf like that

### Packages
Under here will be system wide pacman packages, aur packages or curls what will download and install some app (like oh-my-zsh)
Curl scripts support will be implemented in future updates.

### Users
Here will live the things like users user specifik config (services, scripts, packages, things like that)
The root password will move to the system config, also the eneblement of the root account.
Also in there will live the home and dotfile configs. These configs should be mearge no more seamlesy integrate with each other - so there are not two sepperate parts (they should work as one)

### Built and drivers
Here will be the drivers config, the built type and setting for flags that are passed to the cli.
In future we will support wifi (configs and credential will be there)
Also in future there will be more logging and errors mesages so in this tab there will be config for it.
There should be also the tester part (no more tester preset), you shouold be able to custumazi what the tesster should test, also add a installation test that downlaods a few packages and writes them to run to test this functionality.

## The top bar
In this secton we will describe the top bar.
There should be button for file handeling, reloading, selecting preset, global search (like windows settings), the command pallet and the flash and toml buttons.
The command pallet should be moer usefull and realied on, the work with presets should be easier.
The toml and flash buttons should look differently to the file and presets ones.

### Toml
One you open this tab you switch to edit mode.
If you want to leav you need to supply a valid toml config or discar changes, you can also save mid-way and than discard (that will discard only the unsafe changes.)
Pretty simular to the old one but more easy to use

### Flash
This need lots of work,
We need to add the flash ventoy into the menu like usb/cd/floppy
We can remove the summary or make it more usafful
Also from there it should be easy to make e2e test in qemu - it open qemu and automaticly start the e2e test (like that the suer can what is happening - with graphics)

# End and overwie
The gui should not loose any of its capaibilites and should be a thin wrapper around the toml config and the archstaller cli.
It should make the config of archstaller more easy and enjoyable process not harder and more painfull.
Also create an folder in ./docs in wich there will be list of planed features form this .md - each feature should have its own .md (it should not be implementations details only like list so we do not forget it)