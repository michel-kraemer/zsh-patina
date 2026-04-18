The files in this directory have been taken from the following repository and git tag:

    https://github.com/sublimehq/Packages/tree/v3211

See LICENSE file.

## Changes

Some minor changes have been made to the file `Bash.sublime-syntax` to implement the Zsh syntax.

Show all commits:

```shell
git log -- assets/Packages/ShellScript/Bash.sublime-syntax
```

View the complete diff since the file was first committed:

```shell
FILE=assets/Packages/ShellScript/Bash.sublime-syntax; git diff $(git log --follow --format="%H" -- $FILE | tail -1)..HEAD -- $FILE
```
