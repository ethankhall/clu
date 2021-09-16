# clu

> Program designed to create the perfect world.

Clu is a migration tool, intended to make cross company migrations easier.

## Usage

```bash
> clu help

Clu is a migration tool, intended to make cross company migrations easier

USAGE:
    clu [FLAGS] <SUBCOMMAND>

FLAGS:
    -d, --debug      A level of verbosity, and can be used multiple times
    -e, --error      Disable everything but error logging
    -h, --help       Print help information
    -V, --version    Print version information
    -w, --warn       Enable warn logging

SUBCOMMANDS:
    check-status     Check the status of a migration
    help             Print this message or the help of the given subcommand(s)
    init             Build a default migration toml file
    run-migration    Run a migration, and write the results back to the file
```

## Create a Migration

Using the `init` sub-command will create a `migration.toml` file in your current working directory.

```bash
clu init
```

```toml
[targets]
dummy-repo = 'git@github.com:ethankhall/dummy-repo.git'

[checkout]
branch-name = 'ethankhall/foo-example'
pre-flight = '/usr/bin/true'

[pr]
title = 'Example Title'
description = '''
This is a TOML file

So you can add newlines between the PR's'''

[[steps]]
name = 'Example'
migration-script = 'examples/example-migration.sh'
```

### Targets

The `targets` block contains a map of "pretty names" to "repo path". The pretty name is only used
for reporting, and the path is any valid url to be passed to `git clone <url>`.

The names are required to be unique.

### Checkout

`branch-name` is the name of the branch that will be created and pushed to GitHub. This should be
garenteed to be unique. You should use something like `2021-03-21-upgrade-terraform-to-13`. This
is because if the branch already exists, the migration *WILL FAIL*.

`pre-flight` is a command that will be run to see if the migration needs to be run. This should
be used to help you. Instead of having to manage if a migration is done, and update the list of repos
you should write a script that checks the target repo is already migrated. If this command exists
non-zero the migration for that repo will be skipped.

If you always want the migration to be run, use `/usr/bin/true` which will always return 0.

### PR

`title` is the title of the Pull Request.

`description` is the body that will be put into the PR body. You should include what users should
do with this PR and who to contact with questions. Because this input is TOML, you can use a multiline
string. See the [TOML website](https://toml.io/en/) for more details.

### Steps

This is a list, you can have multiple steps per migration. The PR will only be created after all
steps complete successfully.

`name` is the name of the step, only used for output
`migration-script` is the path to a script that will do the updates for the step of the migration.

When the script it run, the working directory will be the checkout of the repo to work on. If you need
to access files or other details relative to the `migration-script` you could use
`DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"` to get the directory of the script.
`DIR` is the directory that contains the script being run.

When the script is complete, if there are any changes that are left uncommited, the step *WILL FAIL*.
This is a design choice, if there are changes left uncommitted should they be? Instead of having this
ambiguity, the migration will just fail. If you commit the changes OR run `git clean -dfx` then everything
will succeed.

Whenever your dont making changes, *you* must commit them. If you want to have a git message that's very
useful, you should use a file to commit the message by using `git commit -F message.txt`

### Running a Migration 

```bash
clu run-migration --migration-defintion migration.toml
```

`migration.toml` defines the migration. Before starting processing, `clu` will create
a backup of the file next to the existing one. This is because when `clu` is done it will
update `migration.toml` with the current status.

`work-dir` directory will be created, following the following pattern (this directory is tunable
cli argument)

```
work-dir
└── some-repo-name <- This name changes
   ├── repo (clone of the repo)
   ├── stderr.log
   └── stdout.log
```

`work-dir/some-repo-name/stdout.log` is the output from all scripts `standard out`.
`work-dir/some-repo-name/stderr.log` is the output from all scripts `standard error`.
`work-dir/some-repo-name/repo` is the directory that contains the result after the
migration is complete.

### Checking the status of a Migration

After a migration completes the PR status can be checked with

```bash
clu check-status --results migration.toml
```

The CLI will output a markdown styled output to standard out of the status of the migration.
