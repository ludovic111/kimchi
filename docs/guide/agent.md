# The agent

The Agent panel edits the project for you from a request in your own words: "cut the shots on the
beat of the music", "add a lower third with my name at 0:05", "make a 3D title that spins in". It
runs a model you already have, calls kimchi's commands one by one, shows each one, and lets you undo
the whole run at once.

Open it with ⌘J (Ctrl+J) or the **Agent** button in the top bar.

## Choosing who runs it

Choose who runs it in the panel's header, or in **Settings › Agent**. The first choice needs no
setup at all:

**lsuite AI** (the default on a new install). Sign in once with your lsuite account and the agent
works: Claude models on a monthly plan, nothing to install, no key to paste. Press **Sign in**; the
lsuite page opens in your browser, you sign in (or make an account and pick a plan), press
**Connect kimchi**, and you're back in kimchi, signed in. Every lsuite app on the computer is signed
in with it (ryolune, zenith). On a computer without a browser, **Use a key** takes the key your
account page shows (`lsk_…`). Signed in, Settings › Agent shows your plan and how much of the
month's allowance is used (`Pro · 38 % used · resets 1 Nov`), **Manage plan** (your account page)
and **Sign out**. When the allowance runs out, the run stops with one line saying so and a
**Manage plan** button; kimchi never switches to another provider by itself. lsuite AI is a demo
for now: choosing a plan charges nothing.

Or bring your own:

| Choice | What it needs | Model |
| --- | --- | --- |
| Claude Code | The `claude` command installed and signed in (or **Run Claude Code on lsuite AI**, below) | Its own default |
| Codex | The `codex` command installed and signed in | Its own default |
| Anthropic API | An Anthropic API key, saved in Settings › Agent or in `ANTHROPIC_API_KEY` | `claude-sonnet-5-5` |
| OpenAI API | An OpenAI API key, or `OPENAI_API_KEY` | `gpt-5` |
| Ollama | Ollama running on this computer with a model that supports tools (for example `ollama pull qwen3`) | The most recently pulled |
| Zenith | Zenith installed (lsuite); its providers and models appear in the panel | Chosen in Zenith or in the panel |

Gemini CLI, Google Gemini, OpenRouter, Groq, Mistral, DeepSeek, xAI, Together, Fireworks, Cerebras,
Azure OpenAI, Amazon Bedrock, LM Studio and any OpenAI-compatible server are in the list too.

Pick the model in the panel's header: it lists the models lsuite AI, Zenith and Ollama report, and
you can type any model name. Claude Code and Codex use your existing subscription or sign-in. API
keys are billed by the provider per use, separately from any chat subscription. **Model** in
Settings › Agent overrides the default; **Address** points the Anthropic, OpenAI or Ollama choice at
a proxy or another compatible server; an OpenAI-compatible address needs no key. The OpenAI key is
the same one image generation uses.

**Run Claude Code on lsuite AI**: with Claude Code chosen and your lsuite account signed in, this
switch in Settings › Agent makes Claude Code use your lsuite plan instead of its own sign-in.

Settings › Agent shows whether each choice is ready and, if not, why. kimchi looks for `claude`
and `codex` on your login shell's `PATH` and in the usual install folders.

When Claude Code or Codex runs for kimchi, it gets only kimchi's tools: no shell, no file
editing, no web, and none of your own MCP servers.

## Asking

Type in the box at the bottom and press ⌘Enter. While it works, each command it runs appears as a
card with its name, a summary of its parameters, and whether it succeeded; click a card for the
full parameters and answer. **Stop** ends the run and keeps the edits already made.

**Steer it while it works:** type a new direction and send it during a run. The agent takes it into
account from its next step, without losing the edits it already made.

**It knows what "this" is.** Each request carries what you're looking at: the project, the playhead
and what's under it, the selected clips or media, and the clip open in the Studio. The line above
the message box shows what it will be told (hover it for the full text), so "make this shorter" or
"put a title here" works without naming anything.

**It can see.** When it makes a title, an animation or a 3D scene, it renders frames, looks at them
(from any angle in 3D) and corrects what looks wrong before saying it's done. It can also look at
your media (a picture as it is, a video as a sheet of frames) to choose between takes or describe
footage. Every picture it looks at appears in its command's card, so you see what it saw. Models
that can't read pictures get a short text note instead.

A run ends with a summary: how many changes it made, how long it took, and the tokens it used.

### Conversations

Each project keeps its own conversations. **New conversation** (+) starts another; the list in the
panel's header switches between them, and you can rename them. They are saved with your kimchi data
and come back after restarting. **Project memory** is a short text every conversation of the project
shares: preferences, names, the style of the edit. Switching to another project stops a run in
progress; earlier runs can then no longer be reverted from the panel (⌘Z still undoes their steps).
Runs through the APIs or Ollama stop after 40 steps.

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
| Plugins | Write, build, install, remove and switch plugins ([Plugins](plugins.md)) | Off (sending a request from Plugins › Build with your agent turns it on) |

There is no prompt asking you to approve a command: a command that needs a permission that is off
is refused, and the card shows why. Some things are never open to agents: API keys, these
permissions, the choice of model that runs the agent, and signing in or out of lsuite AI. Agents never see your keys (only their
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
