//! The programs the seed gets in `bin/` next to busybox and zig (`docs/build.md`,
//! "The seed"). Their text is part of the seed id.

/// `bin/cc` and `bin/c++` of the seed. `{tool}` is `cc` or `c++`.
///
/// The wrapper adds `VUG_CFLAGS`, and `VUG_LDFLAGS` unless the call only
/// compiles or preprocesses, uses the C++ runtime at `VUG_LIBCXX` in place
/// of the one zig links statically, and turns the GNU and clang spellings that
/// packages use into what zig accepts (`docs/build.md`, "The seed").
pub const COMPILER_WRAPPER: &str = r#"#!/bin/sh
if [ "$#" = 1 ] && [ "$1" = -v ]; then
  exec /seed/zig/zig {tool} -target x86_64-linux-musl -v
fi
link=1
asm=0
dep=0
case " $* " in
  *" -fsyntax-only "*) syntax_only=1 ;;
esac
case " $* " in
  *" -nostdinc "*) nostdinc=1 ;;
esac
set -- "$@" ${VUG_CFLAGS-}
for a; do
  shift
  is_output=0
  if [ "${want_output:-0}" = 1 ]; then
    want_output=0
    output=$a
    is_output=1
  fi
  if [ "${want_retain:-0}" = 1 ]; then
    want_retain=0
    case "$a" in
      -Wl,*,*) a=${a#-Wl,}; retain_file=${a%%,*}; a=-Wl,${a#*,} ;;
      *) retain_file=${a#-Wl,}; continue ;;
    esac
  fi
  if [ -n "${want_value:-}" ]; then
    set -- "$@" "-Wl,$want_value,${a#-Wl,}"
    want_value=
    continue
  fi
  if [ "${want_lang:-0}" = 1 ]; then
    want_lang=0
    if [ "${syntax_only:-0}" = 1 ]; then
      case "$a" in
        c-header) a=c ;;
        c++-header) a=c++ ;;
      esac
    fi
    set -- "$@" "$a"
    continue
  fi
  if [ "${isystem:-0}" = 1 ]; then
    isystem=0
    set -- "$@" "-I$a"
    continue
  fi
  case "$a" in
    --target=* | -mtune=generic) ;;
    -isystem) isystem=1 ;;
    -isystem?*) set -- "$@" "-I${a#-isystem}" ;;
    -m16) set -- "$@" -m16 -Xclang -target-feature -Xclang +16bit-mode ;;
    -mno-sse) set -- "$@" -mno-sse -mno-sse2 -mno-sse3 -mno-ssse3 -mno-sse4.1 -mno-sse4.2 -mno-avx -mno-avx2 ;;
    -march=x86-64) set -- "$@" -march=x86_64 ;;
    -march=x86-64-v[234]) set -- "$@" "-march=x86_64_v${a#-march=x86-64-v}" ;;
    -S) link=0; asm=1; set -- "$@" "$a" ;;
    -E) link=0; preprocess=1; set -- "$@" "$a" ;;
    -c | -M | -MM) link=0; set -- "$@" "$a" ;;
    -MD | -MMD | -Wp,-MD,* | -Wp,-MMD,*) dep=1; set -- "$@" "$a" ;;
    -Wl,*)
      words=
      last=
      IFS=,
      for w in ${a#-Wl,}; do
        if [ "${want_retain:-0}" = 1 ]; then
          want_retain=0
          retain_file=$w
          continue
        fi
        if [ "${skip_word:-0}" = 1 ]; then
          skip_word=0
          continue
        fi
        if [ -n "${take:-}" ]; then
          eval "${take}_fn=\$w"
          take=
          continue
        fi
        last=$w
        case "$w" in
          -init | --init) take=init ;;
          -fini | --fini) take=fini ;;
          -init=* | --init=*) init_fn=${w#*=} ;;
          -fini=* | --fini=*) fini_fn=${w#*=} ;;
          -stats | -lc | -shared | --allow-multiple-definition | --default-symver | -Map=* | --Map=*) ;;
          -Map | --Map) skip_word=1 ;;
          -retain-symbols-file | --retain-symbols-file) want_retain=1 ;;
          -retain-symbols-file=* | --retain-symbols-file=*) retain_file=${w#*=} ;;
          --version-script | --version-script=*) version_script=1; words="$words,$w" ;;
          -T | --script | --script=*) linker_script=1; words="$words,$w" ;;
          -h) words="$words,-soname" ;;
          -h?*) words="$words,-soname,${w#-h}" ;;
          -R) words="$words,-rpath" ;;
          -R?*) words="$words,-rpath,${w#-R}" ;;
          *) words="$words,$w" ;;
        esac
      done
      unset IFS
      case "$last" in
        -rpath | --rpath | -R)
          words=${words%,-rpath}
          want_value=-rpath
          ;;
        -soname | --soname | -h | --version-script | --dynamic-list | -T | --script | -e | --entry)
          words=${words%,*}
          case "$last" in
            -h) want_value=-soname ;;
            *) want_value=$last ;;
          esac
          ;;
      esac
      if [ -n "$words" ]; then set -- "$@" "-Wl$words"; fi
      ;;
    -o) output_given=1; want_output=1; set -- "$@" "$a" ;;
    -) stdin=1; set -- "$@" "$a" ;;
    -x) lang=1; want_lang=1; set -- "$@" "$a" ;;
    -x?*) lang=1; set -- "$@" "$a" ;;
    -shared) shared=1; set -- "$@" "$a" ;;
    -v) verbose=1; set -- "$@" "$a" ;;
    -T) linker_script=1; set -- "$@" "$a" ;;
    -nodefaultlibs | -r | -ffreestanding) no_unwind=1; set -- "$@" "$a" ;;
    -nostdlib) nostdlib=1; set -- "$@" "$a" ;;
    -nostdlib++) nostdlibcxx=1; set -- "$@" "$a" ;;
    -nostdinc++) nostdinccxx=1; set -- "$@" "$a" ;;
    -lc++ | -lstdc++ | -lc++abi | -lunwind)
      if [ "$a" = -lunwind ]; then no_unwind=1; fi
      if [ -n "${VUG_LIBCXX-}" ]; then cxx_libs=1; else set -- "$@" "$a"; fi
      ;;
    -fsyntax-only) link=0 ;;
    -*) set -- "$@" "$a" ;;
    *.o | *.lo | *.a | *.c | *.cc | *.cpp | *.cxx | *.C | *.s | *.S) objects_given=1; set -- "$@" "$a" ;;
    *.h | *.hh | *.hpp | *.i | *.ii | *.m | *.ld | *.lds | *.map | *.sym | *.def | *.txt | *.d) set -- "$@" "$a" ;;
    *)
      if [ "$is_output" = 0 ] && [ -f "$a" ] && {
        [ "${a%.oo}" != "$a" ] ||
        { [ "$(dd if="$a" bs=1 count=4 2>/dev/null | od -An -c | tr -d ' ')" = '177ELF' ] &&
          [ "$(dd if="$a" bs=1 skip=16 count=1 2>/dev/null | od -An -tu1 | tr -d ' ')" = 1 ]; }
      }; then
        if [ -z "${objects:-}" ]; then
          objects=$(mktemp -d) || exit
        fi
        n=$((${n:-0} + 1))
        ln -s "$(realpath "$a")" "$objects/$n.o" || exit
        objects_given=1
        set -- "$@" "$objects/$n.o"
      else
        set -- "$@" "$a"
      fi
      ;;
  esac
done
if [ -n "${VUG_LIBCXX-}" ] && [ "{lang}" = c++ ]; then
  if [ "${nostdlibcxx:-0}" = 0 ]; then
    set -- "$@" -nostdlib++
    if [ "${nostdlib:-0}" = 0 ] || [ "${shared:-0}" = 1 ]; then cxx_libs=1; fi
  fi
  if [ "${nostdinccxx:-0}" = 0 ]; then
    set -- "$@" "-I$VUG_LIBCXX/include/c++/v1"
  fi
fi
if [ "$link" = 1 ] && [ "${cxx_libs:-0}" = 1 ]; then
  if [ "{lang}" = c++ ] && [ "${shared:-0}" = 1 ] && [ "${nostdlib:-0}" = 1 ]; then
    set -- "$@" -Wl,--no-as-needed "$VUG_LIBCXX/lib/libc++.so.1" -Wl,--as-needed
  else
    set -- "$@" "$VUG_LIBCXX/lib/libc++.so.1"
  fi
  set -- "$@" "$VUG_LIBCXX/lib/libc++abi.so.1" "$VUG_LIBCXX/lib/libunwind.so.1" "-Wl,-rpath,$VUG_LIBCXX/lib"
fi
if [ "${syntax_only:-0}" = 1 ]; then
  mkdir -p /tmp/vug-cc-$$
  set -- "$@" -c -o /tmp/vug-cc-$$/syntax.o
fi
if [ "${nostdlib:-0}" = 1 ] && [ "${nostdinc:-0}" = 0 ]; then
  for d in x86_64-linux-musl generic-musl x86-linux-any any-linux-any; do
    set -- "$@" -isystem "/seed/zig/lib/libc/include/$d"
  done
fi
if [ "$link" = 1 ] && [ -n "${init_fn:-}${fini_fn:-}" ]; then
  mkdir -p /tmp/vug-cc-$$
  {
    if [ -n "${init_fn:-}" ]; then
      printf 'extern void %s(void);\n__attribute__((constructor)) static void vug_init(void) { %s(); }\n' "$init_fn" "$init_fn"
    fi
    if [ -n "${fini_fn:-}" ]; then
      printf 'extern void %s(void);\n__attribute__((destructor)) static void vug_fini(void) { %s(); }\n' "$fini_fn" "$fini_fn"
    fi
  } > /tmp/vug-cc-$$/init.c
  objects_given=1
  set -- "$@" /tmp/vug-cc-$$/init.c
fi
set -- "$@" -Wno-error=unused-command-line-argument -Wno-error=date-time
if [ "$link" = 1 ] && [ "${objects_given:-0}" = 0 ] && [ "${stdin:-0}" = 0 ] && [ "${output_given:-0}" = 1 ]; then
  mkdir -p /tmp/vug-cc-$$
  : > /tmp/vug-cc-$$/empty.c
  set -- "$@" /tmp/vug-cc-$$/empty.c
fi
if [ "${stdin:-0}" = 1 ] && [ "${lang:-0}" = 0 ]; then
  set -- -x {lang} "$@"
fi
if [ "$asm" = 1 ] && [ "$dep" = 1 ]; then
  (
    out=
    prev=
    for a; do
      shift
      if [ "$prev" = -o ]; then out=$a; set -- "$@" /dev/null
      elif [ "$a" = -S ]; then set -- "$@" -E
      else set -- "$@" "$a"; fi
      prev=$a
    done
    exec /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" -MT "$out" > /dev/null
  ) || exit
  for a; do
    shift
    case "$a" in
      -MD | -MMD | -Wp,-MD,* | -Wp,-MMD,*) ;;
      *) set -- "$@" "$a" ;;
    esac
  done
fi
if [ "${stdin:-0}" = 1 ] && [ "${preprocess:-0}" = 1 ]; then
  marker='s|"[^"]*zig-local/tmp/[0-9a-f]*-stdin\.c"|"<stdin>"|'
  if [ -n "${output:-}" ]; then
    /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" || exit
    if [ -f "$output" ]; then sed -i "$marker" "$output"; fi
    exit 0
  fi
  /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" > /tmp/vug-cc-$$.i
  status=$?
  sed "$marker" /tmp/vug-cc-$$.i
  rm -f /tmp/vug-cc-$$.i
  exit $status
fi
if [ "$link" = 1 ]; then
  if [ "${want_retain:-0}" = 1 ]; then
    printf '%s\n' 'retain-symbols-file requires a file' >&2
    exit 1
  fi
  if [ "${retain_file+x}" = x ]; then
    retain_option=--version-script
    if [ "${version_script:-0}" = 1 ]; then
      if [ "${linker_script:-0}" = 1 ]; then
        printf '%s\n' 'retain-symbols-file cannot be combined with both --version-script and -T' >&2
        exit 1
      fi
      retain_option=-T
    fi
    retain_script=$(mktemp /tmp/vug-cc-symbols.XXXXXX) || exit
    trap 'rm -f "$retain_script"' EXIT
    LC_ALL=C awk -v option="$retain_option" '
      BEGIN {
        if (option == "-T") print "VERSION {"
        print "{"
      }
      {
        sub(/^[[:space:]]+/, "")
        sub(/[[:space:]]+$/, "")
        if ($0 == "" || $0 ~ /^#/) next
        if (index($0, "\"")) {
          print "retain-symbols-file: a symbol contains a double quote" > "/dev/stderr"
          exit 1
        }
        if (!symbols++) print "  global: extern \"C\" {"
        printf "    \"%s\";\n", $0
      }
      END {
        if (symbols) print "  };"
        print "  local: *;\n};"
        if (option == "-T") print "}"
      }
    ' < "$retain_file" > "$retain_script" || exit
    set -- "$@" "-Wl,$retain_option,$retain_script"
  fi
  set -- "$@" ${VUG_LDFLAGS-}
  if [ "{lang}" = c ] && [ "${nostdlib:-0}${no_unwind:-0}${cxx_libs:-0}" = 000 ] &&
     [ "${objects_given:-0}${stdin:-0}" != 00 ]; then
    set -- "$@" -lunwind
  fi
  if [ "${verbose:-0}" = 1 ]; then
    diagnostics=$(mktemp) || exit
    /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" 2>"$diagnostics"
    status=$?
    awk -v cwd="$PWD/" '
      function in_cache(path, dir) {
        return dir != "" && (index(path, dir "/") == 1 || index(cwd path, dir "/") == 1)
      }
      function from_zig_cache(path) {
        return in_cache(path, ENVIRON["ZIG_GLOBAL_CACHE_DIR"]) ||
               in_cache(path, ENVIRON["ZIG_LOCAL_CACHE_DIR"])
      }
      BEGIN {
        runtime = ENVIRON["VUG_LIBCXX"]
        libraries[runtime "/lib/libc++.so.1"] = "-lc++"
        libraries[runtime "/lib/libc++abi.so.1"] = "-lc++abi"
        libraries[runtime "/lib/libunwind.so.1"] = "-lunwind"
      }
      $1 == "ld.lld" {
        line = $1
        runtime_seen = 0
        for (i = 2; i <= NF; i++) {
          if ($i == "-o" && from_zig_cache($(i + 1))) next
          if (from_zig_cache($i)) continue
          if (runtime != "" && $i in libraries) {
            if (!runtime_seen++) line = line " -L" runtime "/lib"
            line = line " " libraries[$i]
          } else {
            line = line " " $i
          }
        }
        $0 = line
      }
      { print }
    ' "$diagnostics" >&2
    rm -f "$diagnostics"
    exit "$status"
  fi
  if [ -n "${retain_script:-}" ]; then
    /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@"
    exit
  fi
fi
exec /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@"
"#;

/// `bin/ar`, `bin/ranlib`, and `bin/ld` of the seed.
/// `bin/cpp` of the seed: the C preprocessor, reading standard input when
/// no input file is named.
pub const CPP_WRAPPER: &str = r#"#!/bin/sh
for a; do
  case "$a" in
    -) exec /seed/bin/cc -E "$@" ;;
    -*) ;;
    *) exec /seed/bin/cc -E "$@" ;;
  esac
done
exec /seed/bin/cc -E "$@" -
"#;

pub const TOOL_WRAPPER: &str = "#!/bin/sh\nexec /seed/zig/zig {tool} \"$@\"\n";
