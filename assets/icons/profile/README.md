# Zetta profile icons

The Windows `.ico` files are embedded in `zetta-gui.exe` for Jump List entries;
the other files are what the tab bar and settings render. Each shell's icon is
used to identify that shell and does not imply its project endorses Zetta.

## Zetta and Fish

`zetta.svg`, `fish.svg`, and their `.ico` files are original artwork created
for Zetta and are distributed under the same GPL-3.0-or-later license as the
application.

Fish uses original artwork rather than the official fish logo because the logo
has no license of its own. The `><>` artwork (`fish.png` in
[fish-shell](https://github.com/fish-shell/fish-shell)) is covered only by that
repository's `COPYING`, which is GPL-2.0-only. GPL-2.0-only material cannot be
combined into a GPL-3.0-or-later work, and an icon compiled into the binary is
arguably a combination rather than mere aggregation. In [fish-shell
#7915](https://github.com/fish-shell/fish-shell/issues/7915) (April 2021) the
maintainers confirmed that the logo has no written license or usage policy and
gave that one requester permission to use it, while noting that fish needs a
published logo usage document. That permission is specific to the requester's
project and does not extend to Zetta. Replace `fish.svg` and `fish.ico` with
the official logo only once the fish maintainers publish a license or usage
policy for it, or grant Zetta permission, on terms compatible with
GPL-3.0-or-later.

## Bash

`bash.png` (the 32×32 export) and the images inside `bash.ico` (the 16, 24, 32,
48, 64, and 256 px exports, packed without resampling) are the unmodified
[official GNU Bash logo](https://github.com/odb/official-bash-logo) icons, from
`assets/Logos/Icons/PNG/` at commit `e44dab9f89aadd410ff04825b2692eab16711211`.
Copyright 2016 Free Software Foundation. Designed by ProspectOne, art directed
by Justin Dorfman, commissioned by MaxCDN for Chet Ramey on behalf of GNU/FSF.
Distributed under the [Free Art License 1.3](LICENSES/FAL-1.3.txt).

The Free Art License is not GPL-compatible, so this artwork is kept as a
separate, unmodified file under its own license rather than combined into
Zetta's GPL artwork. The upstream SVGs are not used because Illustrator
embedded about 800 KB of private editor data in each. Prefer the upstream
exports over a cleaned or redrawn copy: any change to the artwork falls under
section 2.3 of the license.

## Zsh

`zsh.svg` (`svg/color_logomark.svg`) and `zsh.ico`
(`app-icons/zsh_icon_256x256-multi-size.ico`) are the unmodified [official Zsh
logo](https://github.com/Zsh-art/logo) files at commit
`17617f2f6c70c65943a48745c91d997e7561f19d`. Copyright Justin Dorfman and
contributors. Designed by Guist, art directed by Justin Dorfman, commissioned
by Reblaze. Distributed under [Creative Commons Attribution-ShareAlike 4.0
International](LICENSES/CC-BY-SA-4.0.txt).

## Tux

`tux.png` is copied from [Windows Terminal's WSL profile-generator
asset](https://github.com/microsoft/terminal/blob/main/src/cascadia/CascadiaPackage/ProfileGeneratorIcons/WSL.png),
which is distributed by that project under the [MIT
license](https://github.com/microsoft/terminal/blob/main/LICENSE). Copyright
(c) Microsoft Corporation. All rights reserved. It is used as a generic
Linux/WSL mark and does not identify the shell running inside a distribution.
`tux.ico` is generated from it.
