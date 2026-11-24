source_root=$PWD
cd "$1"
prefix=$2
language=$3
if [ ! -f src/parser.c ]; then
    echo "tree-sitter: missing generated parser: $1/src/parser.c" >&2
    exit 1
fi

work=$(mktemp -d /build/tree-sitter.XXXXXX)
cc -fPIC -Isrc -c src/parser.c -o "$work/parser.o"
set -- "$work/parser.o"
linker=cc
if [ -f src/scanner.c ]; then
    cc -fPIC -Isrc -c src/scanner.c -o "$work/scanner-c.o"
    set -- "$@" "$work/scanner-c.o"
fi
if [ -f src/scanner.cc ]; then
    c++ -fPIC -Isrc -c src/scanner.cc -o "$work/scanner-cxx.o"
    set -- "$@" "$work/scanner-cxx.o"
    linker=c++
fi

mkdir -p "$prefix/lib" "$prefix/parser"
library=libtree-sitter-$language.so
"$linker" -shared -Wl,--no-undefined -o "$prefix/lib/$library" "$@"
ln -s "../lib/$library" "$prefix/parser/$language.so"

queries=queries
if [ ! -d "$queries" ]; then
    queries=$source_root/queries
fi
if [ -d "$queries" ]; then
    mkdir -p "$prefix/queries/$language"
    cp -R "$queries"/. "$prefix/queries/$language/"
fi
