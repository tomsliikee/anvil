#!/bin/sh
# Extracts the translatable strings into po/anvil.pot and merges them into
# every po/<lang>.po. Run inside the GNOME SDK (xgettext knows Rust):
#   tools/sdk.sh tools/update-po.sh
# Command titles, template titles and snippet titles live in tables in
# anvil-core; they are turned into tr("…") calls in a temporary file so
# xgettext sees them.
set -e
cd "$(dirname "$0")/.."
extra=$(mktemp --suffix=.rs)
{
    grep -o 'cmd!("[^"]*", "[^"]*", "[^"]*"' crates/anvil-core/src/commands.rs | sed 's/cmd!("[^"]*", "[^"]*", \("[^"]*"\)/tr(\1);/'
    grep -o 'snip!("[^"]*", "[^"]*"' crates/anvil-core/src/snippets.rs | sed 's/snip!("[^"]*", \("[^"]*"\)/tr(\1);/'
    sed -n '/fn title(self)/,/^    }/p' crates/anvil-core/src/templates.rs | grep -o '=> "[^"]*"' | sed 's/=> \("[^"]*"\)/tr(\1);/'
} > "$extra"
xgettext --language=Rust --from-code=UTF-8 --keyword=tr --keyword=trf --keyword=ntr:1,2 \
    --add-comments=TRANSLATORS --package-name=anvil -o po/anvil.pot \
    $(find anvil/src -name '*.rs' | sort) "$extra"
rm -f "$extra"
for lang in $(cat po/LINGUAS); do
    if [ -f "po/$lang.po" ]; then
        msgmerge --update --backup=none "po/$lang.po" po/anvil.pot
    else
        msginit --no-translator --locale="$lang" -i po/anvil.pot -o "po/$lang.po"
    fi
done
