use strict;
use warnings;
use Config;
use CPAN::Meta;
use File::Path qw(make_path);

my ($out, $driver, $build, $runtime, $jobs, $checks, $modules) = @ARGV;
my $abi = "$Config{version}:$Config{archname}";

sub search_path {
    my (@roots) = @_;
    my @paths;
    for my $root (@roots) {
        my $lib = "$root/lib/perl5";
        next unless -d $lib;
        my $marker = "$lib/.vug-perl-abi";
        if (-f $marker) {
            open my $fh, '<', $marker or die "$marker: $!";
            chomp(my $value = <$fh> // '');
            die "$root: Perl ABI $value does not match $abi\n" unless $value eq $abi;
        }
        push @paths, $lib;
    }
    return join ':', @paths;
}

sub run {
    my (@command) = @_;
    system { $command[0] } @command;
    die "@command failed (status $?)\n" if $? != 0;
}

sub verify_prereqs {
    my ($requirements) = @_;
    for my $module (sort $requirements->required_modules) {
        my $actual = $];
        if ($module ne 'perl') {
            die "Invalid prerequisite name: $module\n"
                unless $module =~ /\A[A-Za-z_]\w*(?:::[A-Za-z_]\w*)*\z/a;
            (my $file = $module) =~ s{::}{/}g;
            require "$file.pm";
            no strict 'refs';
            $actual = $module->VERSION // 0;
        }
        die "$module $actual does not satisfy " . $requirements->requirements_for_module($module) . "\n"
            unless $requirements->accepts_module($module, $actual);
    }
}

my $metadata = -f 'META.json' ? 'META.json' : -f 'META.yml' ? 'META.yml' : undef;
my $run_path = search_path(split /:/, $runtime);
my $build_path = search_path(split /:/, $build);
if (defined $metadata) {
    my $prereqs = CPAN::Meta->load_file($metadata)->effective_prereqs;
    for my $phase ('runtime', 'configure', 'build', ($checks eq 'true' ? 'test' : ())) {
        my $path = $phase eq 'runtime' ? $run_path : $build_path;
        my $pid = fork // die "fork: $!";
        if ($pid == 0) {
            unshift @INC, map { ("$_/$Config{archname}", $_) } grep { length } split /:/, $path;
            verify_prereqs($prereqs->requirements_for($phase, 'requires'));
            exit 0;
        }
        waitpid $pid, 0;
        die "Unsatisfied Perl $phase prerequisites\n" if $? != 0;
    }
}

$ENV{PERL5LIB} = $build_path;
if ($driver eq 'make-maker') {
    run($^X, 'Makefile.PL', "INSTALL_BASE=$out", 'CC=cc', 'LD=cc');
    run('make', "-j$jobs");
    run('make', 'test') if $checks eq 'true';
    run('make', 'install');
} else {
    run($^X, 'Build.PL', '--install_base', $out, '--config', 'cc=cc', '--config', 'ld=cc');
    run($^X, 'Build', 'build');
    run($^X, 'Build', 'test') if $checks eq 'true';
    run($^X, 'Build', 'install');
}

my $lib = "$out/lib/perl5";
make_path($lib);
open my $marker, '>', "$lib/.vug-perl-abi" or die "Perl ABI marker: $!";
print {$marker} "$abi\n";
close $marker or die "Perl ABI marker: $!";

$ENV{PERL5LIB} = join ':', grep { length } ($lib, $run_path);
if ($checks eq 'true') {
    run($^X, "-M$_", '-e', '1') for grep { length } split /,/, $modules;
}

sub quote_shell {
    my ($value) = @_;
    $value =~ s/'/'"'"'/g;
    return "'$value'";
}

if (-d "$out/bin") {
    opendir my $dir, "$out/bin" or die "$out/bin: $!";
    for my $name (sort readdir $dir) {
        my $script = "$out/bin/$name";
        next unless -f $script && !-l $script;
        open my $input, '<', $script or die "$script: $!";
        my $first = <$input> // '';
        close $input;
        next unless $first =~ /\A#!\s*\Q$^X\E(?:\s|\z)/;
        chomp $first;
        my $destination = "$out/libexec/perl/$name";
        make_path("$out/libexec/perl");
        rename $script, $destination or die "$script: $!";
        open my $wrapper, '>', $script or die "$script: $!";
        print {$wrapper} "#!/bin/sh\nexport PERL5LIB=", quote_shell($ENV{PERL5LIB}),
            "\nexec ", quote_shell($destination), " \"\$@\"\n";
        close $wrapper or die "$script: $!";
        chmod 0755, $script or die "$script: $!";
    }
}
