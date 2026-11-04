#!/usr/bin/env perl
# G2's time limit where coreutils `timeout` is absent (#1070): macOS ships
# neither `timeout` nor `gtimeout`, but it ships perl, as Git for Windows and
# every Linux image CI uses do.
#
#   watchdog.pl <grace_s> <limit_s> <command> [<arg>...]
#   watchdog.pl --self-test
#
# The contract is the part of `timeout --kill-after=<grace> <limit>` that
# scripts/gates/test.sh reads. The command runs in its own process group, so
# a test binary cargo started is stopped with it. At the limit the group gets
# TERM, and KILL `grace` seconds later if the command has not exited. Exit
# codes are timeout's: 124 on an expiry, 137 when KILL was needed, 125 on a
# bad argument, 126 or 127 when the command cannot be run, 128+n when signal
# n ended it before the limit, and the command's own code otherwise.
use strict;
use warnings;
use POSIX ();
use Errno ();

sub usage {
    print STDERR "usage: watchdog.pl <grace_s> <limit_s> <command> [<arg>...] | --self-test\n";
    exit 125;
}

sub run {
    my ($grace, $limit, @cmd) = @_;
    # Whole seconds above 0, as test.sh holds GATE_TEST_TIMEOUT_S to. A grace
    # of 0 would mean "never KILL" to coreutils; nothing here needs that.
    usage() unless defined $limit && @cmd;
    usage() unless $grace =~ /\A[1-9][0-9]*\z/ && $limit =~ /\A[1-9][0-9]*\z/;

    my $pid = fork();
    if (!defined $pid) {
        print STDERR "watchdog.pl: fork: $!\n";
        exit 125;
    }
    if ($pid == 0) {
        setpgrp(0, 0);
        { exec { $cmd[0] } @cmd; }
        my $code = $!{ENOENT} ? 127 : 126;
        print STDERR "watchdog.pl: cannot run $cmd[0]: $!\n";
        POSIX::_exit($code);
    }
    # Also from this side: a signal forwarded before the child's own
    # setpgrp ran would otherwise name a group that does not exist yet. It
    # fails harmlessly once the child has exec'd, by which time it is set.
    setpgrp($pid, $pid);

    my $expired = 0;
    my $killed = 0;
    local $SIG{ALRM} = sub {
        if (!$expired) {
            $expired = 1;
            kill 'TERM', -$pid;
            alarm $grace;
        } else {
            $killed = 1;
            kill 'KILL', -$pid;
        }
    };
    # An interrupt or a TERM meant for the gate reaches the command too, as
    # it does through coreutils `timeout`; the group is not the terminal's.
    local $SIG{INT}  = sub { kill 'INT',  -$pid };
    local $SIG{TERM} = sub { kill 'TERM', -$pid };
    local $SIG{HUP}  = sub { kill 'HUP',  -$pid };
    alarm $limit;
    # perl retries waitpid after a handler runs; the loop is for any build
    # that returns EINTR instead.
    my $r;
    do { $r = waitpid($pid, 0) } while ($r == -1 && $!{EINTR});
    my $status = $?;
    alarm 0;
    if ($r != $pid) {
        print STDERR "watchdog.pl: waitpid: $!\n";
        return 125;
    }
    return 137 if $killed;
    return 124 if $expired;
    return 128 + ($status & 127) if $status & 127;
    return $status >> 8;
}

# Runs this script as a separate process and returns (code, seconds).
sub timed {
    my @argv = @_;
    my $t0 = time;
    my $pid = fork();
    die "self-test: fork: $!\n" unless defined $pid;
    if ($pid == 0) {
        open STDERR, '>', '/dev/null';
        { exec { $^X } $^X, $0, @argv; }
        POSIX::_exit(99);
    }
    waitpid($pid, 0);
    my $code = $? & 127 ? 128 + ($? & 127) : $? >> 8;
    return ($code, time - $t0);
}

sub self_test {
    my @fail;
    my $check = sub {
        my ($name, $want, $max_s, @argv) = @_;
        my ($code, $took) = timed(@argv);
        push @fail, "$name: exit $code, want $want" if $code != $want;
        push @fail, "$name: took ${took}s, want at most ${max_s}s" if $took > $max_s;
    };
    my $sh = '/bin/sh';
    $check->('exit 0 passes through', 0, 5, 1, 5, $sh, '-c', 'exit 0');
    $check->('a failing code passes through', 7, 5, 1, 5, $sh, '-c', 'exit 7');
    $check->('an expiry exits 124', 124, 6, 1, 1, $sh, '-c', 'sleep 30');
    # A command that ignores TERM (a cargo that will not stop) ends by KILL
    # within the grace period; without it the gate would hang for the
    # sleep, which is what #173 put the limit there to prevent.
    $check->('TERM ignored: KILL after the grace, 137', 137, 8, 1, 1,
        $^X, '-e', '$SIG{TERM} = "IGNORE"; sleep 30');
    $check->('a signal exits 128+n', 128 + 15, 5, 1, 5, $sh, '-c', 'kill -TERM $$; sleep 5');
    $check->('a missing command exits 127', 127, 5, 1, 5, '/nonexistent/watchdog-self-test');
    $check->('a limit of 0 is refused, 125', 125, 5, 1, 0, $sh, '-c', 'exit 0');
    $check->('a word for a limit is refused, 125', 125, 5, 1, 'soon', $sh, '-c', 'exit 0');
    $check->('no command is refused, 125', 125, 5, 1, 5);

    # A grandchild in the group (a test binary under cargo) dies with the
    # command, rather than outliving the gate and holding the target dir.
    my $dir = $ENV{TMPDIR} || '/tmp';
    my $pidfile = "$dir/watchdog-self-test.$$";
    unlink $pidfile;
    my ($code) = timed(1, 1, $sh, '-c', "sleep 30 & echo \$! > '$pidfile'; wait");
    push @fail, "grandchild: exit $code, want 124" if $code != 124;
    if (open my $fh, '<', $pidfile) {
        my $gpid = <$fh>;
        close $fh;
        chomp $gpid;
        # Give the KILL, if one was needed, time to land.
        my $alive = 1;
        for (1 .. 20) {
            $alive = kill(0, $gpid) ? 1 : 0;
            last unless $alive;
            select(undef, undef, undef, 0.1);
        }
        if ($alive) {
            push @fail, "grandchild $gpid still runs after the expiry";
            kill 'KILL', $gpid;
        }
    } else {
        push @fail, "grandchild: no pid file at $pidfile";
    }
    unlink $pidfile;

    if (@fail) {
        print "FAIL: watchdog.pl self-test\n";
        print "  $_\n" for @fail;
        exit 1;
    }
    print "OK: watchdog.pl self-test (exit codes passed through, 124 on expiry, 137 after TERM ignored, grandchild killed, 128+n on a signal, 127 on a missing command, 125 on a bad argument)\n";
    exit 0;
}

if (@ARGV == 1 && $ARGV[0] eq '--self-test') {
    self_test();
}
exit run(@ARGV);
