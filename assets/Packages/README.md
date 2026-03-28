The files in this directory have been taken from the following repository and
git tag:

    https://github.com/sublimehq/Packages/tree/v3211

See LICENSE file.

## Changes

Some minor changes have been made to implement the Zsh syntax.

Bash.sublime-syntax:

* `constant.character.escape.shell` scope:
  * Added `\uNNNN` and `\UNNNNNNNN` escape sequences
  * Limited `\xNN` escape sequence to two hex characters
* Add `keyword.control.repeat.shell` scope
* Add `repeat-args` context
