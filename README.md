# QEMU test runner
This is a test runner for students' solutions to the OS course assignments.
The assignments require students to modify the kernel of the MINIX3 operating system, which is traditionally emulated with [QEMU](https://www.qemu.org/).
This program builds new copy-on-write images using students' patches, runs tests in isolated environments (each test is executed inside a separate QEMU machine) and generates detailed reports from the testing process.

This runner uses UNIX-specific features of the [Tokio](https://tokio.rs/) crate.

# Dependencies
This program uses the [ssh2](https://docs.rs/ssh2/latest/ssh2/index.html) crate, which contains Rust bindings to the [libssh2](https://www.libssh2.org/) library. You will need its dev package to compile this runner.

# Usage
The most convenient way to compile it uses [Cargo](https://github.com/rust-lang/cargo), which is by default distributed with Rust.
```
cargo build --release
```
This command executed in the root directory of the project will compile the program in release mode. The executable will be located in the `target` directory.

Running the program requires preparing a suite file (its format is described in a separate section below) and a base MINIX3 image for QEMU (raw).
```
path/to/executable --suite=path/to/suite.json --base-image=path/to/minix/image.img
```
The running program will read paths to the solution patches from the STDIN, each in a separate line. File name must be of format given with regex `[a-z]{2}[0-9]{6}\.patch`. The first 8 characters from the file name are a student's identifier. Duplicates will be rejected.

The program uses the [env_logger](https://docs.rs/env_logger/latest/env_logger/) crate to log errors and diagnostical information to the STDERR. This behaviour can be customized using environment variables (see crate's documentation for a detailed guide). Most basic configuration requires the user to set the log level in the `RUST_LOG` variable. Available levels include `trace`, `debug`, `info`, `warn`, `error`. If the log level is not set, all logging is disabled.
```
RUST_LOG=info
```

The program outputs test results to STDOUT in the CSV format:
```
/path/to/solution/1;OK
/path/to/solution/2;build failed
/path/to/solution/3;comma,separated,list,of,failed,tests
```

Additional arguments enable using custom QEMU commands, customizing the emulated environment, increasing the number of concurrent QEMU processes, generating detailed reports and preserving copy-on-write images. For more info run
```
path/to/executable --help
```

# Safety
This program does not implement a custom signal handling. Killing it with a signal may leave leftover QEMU processes.

# Suite configuration
Suite configuration is parsed from a JSON file. It is a JSON object containing:
1. `user` - string, username that will be used for authentication over SSH. Not required, defaults to `root`.
2. `password` - string, password that will be used for authentication over SSH. Not required, defaults to `root`.
3. `ssh_timeout_ms` - number, limit for the time passed from the moment the QEMU process is spawned to the moment the SSH connection is established (milliseconds). Not required, defaults to `20000`.
4. `poweroff_timeout_ms` - number, limit for the time passed from the moment the poweroff is requested to the moment the QEMU process exits (milliseconds). Not required, defaults to `20000`.
5. `poweroff_command` - string, command that will be used to request a poweroff through SSH. Not requried, defaults to `/sbin/poweroff`.
6. `retries` - number, default value for allowed scenario retries. Not required, defaults to `3`.
7. `step_timeout_ms` - number, default value for a single step timeout. Not required, defaults to `5000`.
8. `build` - build scenario. Not required.
9. `tests` - a test name to scenario mapping.
10. `output_limit` - number, limit for STDOUT and STDERR of a single step (outputs will be truncated). Not required.

Example suite configurations can be found in the `examples` directory.

## Scenario configuration
Scenario configuration is a JSON object containing:
1. `retries` - number, allowed scenario retries in case of failure. Not required, defaults to the `retries` value from the suite configuration.
2. `steps` - a list of lists. Each inner lists contains a sequence of steps to be executed. The system will be shut down (using the `poweroff_command` from the suite configuration) and booted in between these inner sequences. The execution of a scenario is stopped after the first failed step.

## Step
Step configuration is a JSON object containing:
1. `type` - string, one of `file_transfer`, `patch_transfer`, `command`.
2. `timeout_ms` - number, time limit for executing this step (milliseconds). Not required, defaults to the `step_timeout_ms` value from the suite configuration.
3. `command` - string, a command to execute over SSH. Exiting with a non-zero code means failure. Only for the `command` type.
4. `from` - string, path (absolute or relative to the parent directory of the suite file) to the local file to send over SSH. Only for the `file_transfer` types.
5. `to` - string, path (absolute or relative to the parent directory of the suite file) to the destination file on the guest system. Only for the `file_transfer` and `patch_transfer` types. The destination file will have permissions set to `0o777`.

## Example build scenario
Disclaimer - this example is not a valid JSON, as JSON is a data-only format and does not allow comments. Here comments begin with `#`.
```
{
    "retries": 1,
    "steps": [
        # A QEMU process is spawned.
        [
            {
                # The student's patch is transferred to MINIX3.
                # The patch is placed under solution.patch in the user's home directory.
                # No timeout is specified for this step, the value from the suite configuration is used.
                "type": "patch_transfer",
                "to": "solution.patch"
            },
            {
                # File build.sh from the same directory as the configuration file is transferred to MINIX3.
                "type": "file_transfer",
                "from": "./build.sh",
                "to": "build.sh"
            },
            {
                # The build script is executed inside MINIX3.
                "type": "command",
                "command": "./build.sh",
                "timeout_ms": 20000
            }
        ],
        # Poweroff is requested.
        # The QEMU process exits.
        # A new QEMU process is spawned using the same image.
        [
            {
                # File tests archive from the absolute path is transferred to MINIX3.
                "type": "file_transfer",
                "from": "/path/to/tests.zip",
                "to": "tests.zip"
            },
            {
                "type": "command",
                "command": "unzip tests.zip && cd tests && make",
                "timeout_ms": 5000
            }
        ]
        # Poweroff is requested.
        # The QEMU process exits.
        # The build process finished successfuly, creating a copy-on-write image. This image will be shared as a backing file by all copy-on-write images used in tests of this solution. 
    ]
}
```

# Tests
A subset of tests can by run with a simple command:
```
cargo test
```
However, some tests are ignored by default. Running them requires setting some environment variables:
1. `TEST_BASE_IMAGE` - path to the base MINIX3 image.
2. `TEST_RUN_CMD` - command used to spawn a QEMU process (for example `qemu-system-x86_64`).
3. `TEST_BUILD_CMD` - command to used to create a copy-on-write image (for example `qemu-img`).
4. `TEST_ENABLE_KVM` - whether to use KVM in tests.

Those tests can by run with a command:
```
cargo test -- --ignored
```
