#!/bin/bash
# First-boot script of the sway preset (configs/sway.lua): installs a small Sway config as
# ~/.config/sway/config for every regular user and into /etc/skel for users added later. An existing
# config (for example one from user_archives) is left alone. Edit the file after the first boot as usual.
set -u

conf=$(mktemp)
trap 'rm -f "$conf"' EXIT
cat > "$conf" <<'EOF'
# Sway config written by the archstaller sway preset. `man 5 sway`; the stock config is /etc/sway/config.
set $mod Mod4
set $left h
set $down j
set $up k
set $right l
set $term foot
set $menu wofi --show drun
set $lock swaylock -f -c 1e1e2e

# Every output, whatever its name or resolution, gets a plain background.
output * bg #1e1e2e solid_color

# Lock on idle. Uncomment after checking that locking and resume work on this machine.
# exec swayidle -w \
#     timeout 300 '$lock' \
#     timeout 600 'swaymsg "output * power off"' resume 'swaymsg "output * power on"' \
#     before-sleep '$lock'

# Notifications. `exec` (not exec_always) runs once per session, so reloading does not start another.
exec mako

floating_modifier $mod normal
bindsym $mod+Return exec $term
bindsym $mod+d exec $menu
bindsym $mod+Shift+q kill
bindsym $mod+Shift+c reload
bindsym $mod+Shift+e exec swaynag -t warning -m 'Exit sway?' -B 'Exit' 'swaymsg exit'
bindsym $mod+Ctrl+l exec $lock

bindsym $mod+$left focus left
bindsym $mod+$down focus down
bindsym $mod+$up focus up
bindsym $mod+$right focus right
bindsym $mod+Left focus left
bindsym $mod+Down focus down
bindsym $mod+Up focus up
bindsym $mod+Right focus right
bindsym $mod+Shift+$left move left
bindsym $mod+Shift+$down move down
bindsym $mod+Shift+$up move up
bindsym $mod+Shift+$right move right
bindsym $mod+Shift+Left move left
bindsym $mod+Shift+Down move down
bindsym $mod+Shift+Up move up
bindsym $mod+Shift+Right move right

bindsym $mod+1 workspace number 1
bindsym $mod+2 workspace number 2
bindsym $mod+3 workspace number 3
bindsym $mod+4 workspace number 4
bindsym $mod+5 workspace number 5
bindsym $mod+6 workspace number 6
bindsym $mod+7 workspace number 7
bindsym $mod+8 workspace number 8
bindsym $mod+9 workspace number 9
bindsym $mod+0 workspace number 10
bindsym $mod+Shift+1 move container to workspace number 1
bindsym $mod+Shift+2 move container to workspace number 2
bindsym $mod+Shift+3 move container to workspace number 3
bindsym $mod+Shift+4 move container to workspace number 4
bindsym $mod+Shift+5 move container to workspace number 5
bindsym $mod+Shift+6 move container to workspace number 6
bindsym $mod+Shift+7 move container to workspace number 7
bindsym $mod+Shift+8 move container to workspace number 8
bindsym $mod+Shift+9 move container to workspace number 9
bindsym $mod+Shift+0 move container to workspace number 10

bindsym $mod+b splith
bindsym $mod+v splitv
bindsym $mod+s layout stacking
bindsym $mod+w layout tabbed
bindsym $mod+e layout toggle split
bindsym $mod+f fullscreen
bindsym $mod+Shift+space floating toggle
bindsym $mod+space focus mode_toggle
bindsym $mod+a focus parent
bindsym $mod+Shift+minus move scratchpad
bindsym $mod+minus scratchpad show

mode "resize" {
    bindsym $left resize shrink width 10px
    bindsym $down resize grow height 10px
    bindsym $up resize shrink height 10px
    bindsym $right resize grow width 10px
    bindsym Left resize shrink width 10px
    bindsym Down resize grow height 10px
    bindsym Up resize shrink height 10px
    bindsym Right resize grow width 10px
    bindsym Return mode "default"
    bindsym Escape mode "default"
}
bindsym $mod+r mode "resize"

# Screenshots: the whole screen, or a selected area, to the clipboard.
bindsym Print exec grim - | wl-copy
bindsym $mod+Shift+s exec grim -g "$(slurp)" - | wl-copy

# Laptop brightness and media keys. On a machine without these keys or devices nothing happens.
bindsym XF86MonBrightnessUp exec brightnessctl set 5%+
bindsym XF86MonBrightnessDown exec brightnessctl set 5%-
bindsym XF86AudioRaiseVolume exec wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+
bindsym XF86AudioLowerVolume exec wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-
bindsym XF86AudioMute exec wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle
bindsym XF86AudioMicMute exec wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle
bindsym XF86AudioPlay exec playerctl play-pause
bindsym XF86AudioNext exec playerctl next
bindsym XF86AudioPrev exec playerctl previous

# Sway starts and restarts waybar itself, so reloading the config never runs a second bar.
bar {
    swaybar_command waybar
}

# The Arch package's drop-ins: import the session environment into systemd and D-Bus (portals need it).
include /etc/sway/config.d/*
EOF

install -D -m 0644 "$conf" /etc/skel/.config/sway/config

getent passwd | while IFS=: read -r name _ uid _ _ home shell; do
    [ "$uid" -ge 1000 ] && [ "$uid" -lt 60000 ] || continue
    case "$shell" in */nologin|*/false) continue ;; esac
    [ -d "$home" ] || continue
    if [ -e "$home/.config/sway/config" ]; then
        echo "sway: $name already has ~/.config/sway/config, left alone"
        continue
    fi
    grp=$(id -gn "$name")
    runuser -u "$name" -- mkdir -p "$home/.config/sway" &&
        install -o "$name" -g "$grp" -m 0644 "$conf" "$home/.config/sway/config" &&
        echo "sway: wrote ~/.config/sway/config for $name" ||
        echo "warning: sway: could not write the config for $name"
done
