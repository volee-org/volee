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
- volee launches every agent with an explicit permission mode: `manual`
  unless you configure another, and never `bypassPermissions`.
- volee never deletes branches.
- Cleanup of worktrees and branches requires evidence that the work was
  delivered.

If you find a way to break one of these properties, please report it as a
vulnerability.

### What volee trusts

These follow from the design and are not vulnerabilities:

- **Every process that runs as you.** The daemon accepts any process that
  can open its socket, and takes the caller's word for who it acts for. Any
  program you run can therefore act as you: send messages in your name, answer
  agents' questions, launch, stop or tidy agents, and abort or close
  missions. That includes a command an agent runs with its shell tool, such
  as `volee msg send` or `volee mission close --force`. The MCP server volee
  gives an agent always acts for that agent; only a process that talks to
  the socket directly can claim another identity. Guard the socket as you
  guard your shell.
- **The MCP tools of the agents volee launches.** Their settings allow every
  volee tool (`mcp__volee`) without a permission prompt, so that a background
  agent never stalls on asking to send a message or a report. A mission's
  coordinator can therefore launch workers, and close its mission once
  volee sees every worker delivered, without asking you. It launches at
  most `[missions] max_workers` agents (8 by default), only in the
  repositories the mission names. The workers it launches get the same
  permission mode and tools as any agent volee launches.
