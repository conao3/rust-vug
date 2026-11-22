//! The programs the seed gets in `bin/` next to busybox and zig (`docs/build.md`,
//! "The seed"). Their text is part of the seed id.

/// `bin/cc` and `bin/c++` of the seed. `{tool}` is `cc` or `c++`.
///
/// The wrapper adds `VUG_CFLAGS`, and `VUG_LDFLAGS` unless the call only
/// compiles or preprocesses, uses the C++ runtime at `VUG_LIBCXX` in place
/// of the one zig links statically, and turns the GNU and clang spellings that
/// packages use into what zig accepts (`docs/build.md`, "The seed").
pub const COMPILER_WRAPPER: &str = r#"#!/bin/sh
link=1
asm=0
dep=0
case " $* " in
  *" -fsyntax-only "*) syntax_only=1 ;;
esac
case " $* " in
  *" -nostdinc "*) nostdinc=1 ;;
esac
for a; do
  shift
  is_output=0
  if [ "${want_output:-0}" = 1 ]; then
    want_output=0
    output=$a
    is_output=1
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
          -stats | -lc | -shared | --allow-multiple-definition | -Map=* | --Map=*) ;;
          -Map | --Map) skip_word=1 ;;
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
    -nostdlib) nostdlib=1; set -- "$@" "$a" ;;
    -nostdlib++) nostdlibcxx=1; set -- "$@" "$a" ;;
    -nostdinc++) nostdinccxx=1; set -- "$@" "$a" ;;
    -lc++ | -lstdc++ | -lc++abi)
      if [ -n "${VUG_LIBCXX-}" ]; then cxx_libs=1; else set -- "$@" "$a"; fi
      ;;
    -fsyntax-only) link=0 ;;
    -*) set -- "$@" "$a" ;;
    *.o | *.lo | *.a | *.c | *.cc | *.cpp | *.cxx | *.C | *.s | *.S) objects_given=1; set -- "$@" "$a" ;;
    *.h | *.hh | *.hpp | *.i | *.ii | *.m | *.ld | *.lds | *.map | *.sym | *.def | *.txt | *.d) set -- "$@" "$a" ;;
    *)
      if [ "$is_output" = 0 ] && [ -f "$a" ] && [ "$(dd if="$a" bs=1 count=4 2>/dev/null | od -An -c | tr -d ' ')" = '177ELF' ] && [ "$(dd if="$a" bs=1 skip=16 count=1 2>/dev/null | od -An -tu1 | tr -d ' ')" = 1 ]; then
        objects=/tmp/vug-cc-$$
        mkdir -p "$objects"
        n=$((${n:-0} + 1))
        ln -s "$(realpath "$a")" "$objects/$n.o"
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
    if [ "${nostdlib:-0}" = 0 ]; then cxx_libs=1; fi
  fi
  if [ "${nostdinccxx:-0}" = 0 ]; then
    set -- "$@" "-I$VUG_LIBCXX/include/c++/v1"
  fi
fi
if [ "$link" = 1 ] && [ "${cxx_libs:-0}" = 1 ]; then
  set -- "$@" "$VUG_LIBCXX/lib/libc++.so.1" "$VUG_LIBCXX/lib/libc++abi.so.1" "$VUG_LIBCXX/lib/libunwind.so.1" "-Wl,-rpath,$VUG_LIBCXX/lib"
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
    exec /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-} -MT "$out" > /dev/null
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
    /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-} || exit
    if [ -f "$output" ]; then sed -i "$marker" "$output"; fi
    exit 0
  fi
  /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-} > /tmp/vug-cc-$$.i
  status=$?
  sed "$marker" /tmp/vug-cc-$$.i
  rm -f /tmp/vug-cc-$$.i
  exit $status
fi
if [ "$link" = 1 ]; then
  exec /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-} ${VUG_LDFLAGS-}
fi
exec /seed/zig/zig {tool} -target x86_64-linux-musl -fno-sanitize=undefined -UNDEBUG "$@" ${VUG_CFLAGS-}
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
