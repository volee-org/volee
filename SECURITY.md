# Security policy

## Supported versions

volee is in early development. During the 0.x series, only the latest release
receives security fixes.

| Version        | Supported |
| -------------- | --------- |
| Latest release | Yes       |
| Older releases | No        |

## Reporting a vulnerability

Please do not open a public issue, discussion or pull request for a security
problem.

Use GitHub private vulnerability reporting instead:

1. Go to https://github.com/volee-org/volee/security/advisories/new (or open the
   Security tab of the repository and choose "Report a vulnerability").
2. Fill in the form and submit it. Only the maintainers can see it.

### What to include

- The volee version (`volee --version`), your OS and how you installed volee.
- A description of the problem and its impact.
- Steps to reproduce, or a proof of concept if you have one.
- Any suggested fix or mitigation.

### Response targets

volee is maintained by volunteers, so these are best-effort targets:

- Acknowledgement within 3 days.
- First assessment (accepted, need more information, or declined) within 10 days.
- We will keep you updated while a fix is prepared.

## Disclosure policy

We follow coordinated disclosure. We will work with you on a fix, publish it in
a new release, and then publish a GitHub security advisory describing the issue.
We credit reporters in the advisory unless you prefer to stay anonymous. Please
give us a reasonable amount of time to ship a fix before you disclose publicly.

## Security model

- The daemon listens on a Unix socket that only the current user can read and
  write.
- volee never edits your Claude Code settings.
- volee launches agents with explicit permission modes. It never uses bypass
  mode by default.
- volee never deletes branches.
- Cleanup of worktrees and branches requires evidence that the work was
  delivered.

If you find a way to break one of these properties, please report it as a
vulnerability.
