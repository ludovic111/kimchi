# The agent

The Agent panel edits the project for you from a request in your own words: "cut the shots on the
beat of the music", "add a lower third with my name at 0:05", "make a 3D title that spins in". It
runs a model you already have, calls kimchi's commands one by one, shows each one, and lets you undo
the whole run at once.

Open it with ⌘J (Ctrl+J) or the **Agent** button in the top bar.

## Choosing who runs it

Choose who runs it in the panel's header, or in **Settings › Agent**:

| Choice | What it needs | Model |
| --- | --- | --- |
| Claude Code (default) | The `claude` command installed and signed in | Its own default |
| Codex | The `codex` command installed and signed in | Its own default |
| Anthropic API | An Anthropic API key, saved in Settings › Agent or in `ANTHROPIC_API_KEY` | `claude-sonnet-5-5` |
| OpenAI API | An OpenAI API key, or `OPENAI_API_KEY` | `gpt-5` |
| Ollama | Ollama running on this computer with a model that supports tools (for example `ollama pull qwen3`) | The most recently pulled |

Claude Code and Codex use your existing subscription or sign-in. API keys are billed by the provider
per use, separately from any chat subscription. **Model** in Settings › Agent overrides the default;
**Address** points the Anthropic, OpenAI or Ollama choice at a proxy or another compatible server;
an OpenAI-compatible address needs no key. The OpenAI key is the same one image generation uses.

Settings › Agent shows whether each choice is ready and, if not, why. kimchi looks for `claude`
and `codex` on your login shell's `PATH` and in the usual install folders.

When Claude Code or Codex runs for kimchi, it gets only kimchi's tools: no shell, no file
editing, no web, and none of your own MCP servers.

## Asking

Type in the box at the bottom and press ⌘Enter. While it works, each command it runs appears as a
card with its name, a summary of its parameters, and whether it succeeded; click a card for the
full parameters and answer. **Stop** ends the run and keeps the edits already made.

A run ends with a summary: how many changes it made, how long it took, and the tokens it used.
The conversation continues across requests until **New conversation** (+). Switching to another
project stops a run in progress and starts afresh: earlier runs can then no longer be reverted from
the panel (⌘Z still undoes their steps). Conversations aren't saved. Runs through the APIs or Ollama
stop after 40 steps.

For motion graphics and 3D, the agent reads kimchi's motion guide, writes the scene, renders frames
to look at them (from any angle in 3D), and corrects what it sees.

## Undoing a run

Every edit the agent makes is an ordinary undo step, so ⌘Z undoes them one by one. **Revert this
run**, under the run's summary, puts the project back as it was when the run started, as a single
step; ⌘Z after that brings it all back. Everything done after that point is reverted too, including
your own edits and later runs, so revert right after a run you don't want.

### Changes

The panel's **Changes** tab shows:

- the runs from this panel, each with **Revert this run**;
- sessions from terminal agents (Claude Code or Codex connected through MCP, or scripts), each with
  **Revert this session**;
- the undo history, with the agent's steps listed one by one and your own edits folded together.

## Permissions

**Settings › Agent › Permissions** decides what agents may do in kimchi: this panel and every MCP
client alike. Editing the open project is always allowed.

| Permission | Allows | Default |
| --- | --- | --- |
| Let agents act in kimchi | Anything at all; off refuses every request | On |
| Files | Import media, export, write files, save a copy of the project | On |
| Projects | Create, open, close, duplicate or delete projects | On |
| Generate | Generate images and video (spends the provider's credits), and ask the built-in agent from a script or MCP | On |
| Settings | Change settings other than these permissions and API keys | Off |
| App control | Quit or restart kimchi, install an update | Off |

There is no prompt asking you to approve a command: a command that needs a permission that is off
is refused, and the card shows why. Some things are never open to agents: API keys, these
permissions, and the choice of model that runs the agent. Agents never see your keys (only their
last four characters).

Permissions apply to agents, not to scripts: plain `kimchi-cli` runs any command. A terminal agent
that can run shell commands could call `kimchi-cli` itself, so permissions don't contain an agent
that has a shell. Scripts that run on an agent's behalf should use `kimchi-cli --agent`, which is held
to the same permissions.

## Using your own agent instead

Claude Code, Codex, Cursor, Claude Desktop and any other MCP client can drive kimchi directly.
**Settings › About & AI control** has the lines to copy, for example:

```sh
claude mcp add kimchi -- /Applications/kimchi.app/Contents/MacOS/kimchi-mcp --live
```

Their edits share the same undo history and permissions, and appear in the Changes tab. The panel's
empty state has the same lines, and `kimchi-cli mcp-config` prints a configuration file. Scripts
and MCP clients can also talk to this panel's agent with `agent.send`, `agent.status` and
`agent.revert`. See
[Controlling kimchi from AI and scripts](../AI_CONTROL.md).
