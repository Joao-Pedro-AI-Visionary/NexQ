# Using Your Claude Subscription (Beta)

NexQ can use your own Claude subscription (Pro or Max) as its AI provider, with no API key. NexQ launches the official `claude` command-line tool installed on your PC and talks to it locally.

> **Beta.** Uses your own Claude login on this PC. NexQ never sees your credentials.

## Setup

1. **Install Claude Code.** Follow the official instructions at [code.claude.com/docs/en/setup](https://code.claude.com/docs/en/setup). NexQ finds the `claude` command automatically once it is installed.
2. **Log in, in your own terminal.** Open a terminal, run `claude`, and sign in with your Claude account when prompted (`/login`). You only need to do this once.
3. **Choose Claude in NexQ.** Open Settings (`Ctrl+,`) > LLM > **Subscription (local CLI)** > **Claude**, then click **Check connection**. A green dot shows the detected path, version, and the account you are signed in with.

If NexQ reports that you are not logged in, click **Open terminal to log in**. This opens a normal terminal window running `claude`. You sign in there, not inside NexQ.

## Fast and Deep models

You choose a model and an effort level for two kinds of work:

| Tier | Used for | Default |
|------|----------|---------|
| **Fast** | Live meeting answers: Assist, What to Say, Shorten, Follow-Up, Ask | Haiku · Low |
| **Deep** | Recap and past-meeting work: summaries, action items, Ask this meeting | Sonnet · Medium |

Haiku is the quickest and lightest on your usage limits. Sonnet reasons more carefully. You can pick any available model (Haiku, Sonnet, Opus, Fable) and a higher effort level for either tier, at the cost of slower answers and more usage. Changes apply to the next session; a running session shows "applies to next session".

## How sessions work

NexQ keeps a Claude session open and sends it only what is new, so answers stay fast and you do not pay to re-send the whole meeting every time.

- **Live meeting.** When you start a meeting, NexQ starts a Fast session in the background. It holds your scenario prompt, custom instructions, and your context documents (up to roughly 100,000 tokens; beyond that, NexQ falls back to normal document search for each question). Each time you use an AI action, NexQ sends only the transcript since your previous request plus the question. The footer shows "Session ready" when it is warmed up. Recap uses a separate Deep session, started the first time you use it. Sessions close when the meeting ends.
- **Past meeting.** Opening a past meeting starts a Deep session preloaded with the transcript (speakers and timestamps) and the summary if there is one. Context documents are not included. The session closes when you leave the meeting or after 15 minutes of inactivity. Reopening restarts it quickly.
- **Ask this meeting.** In a past meeting, the **Ask** tab lets you chat with the meeting. Questions and answers are saved with the meeting, restored when you reopen it, and listed in the AI Log, so follow-up questions keep their context.
- **Other uses.** Things that are not tied to a meeting (such as LLM translation and testing your knowledge base) use their own small Fast session, so they never mix into a meeting.

Some per-action options do not apply in Claude mode (transcript window, temperature, web search), because the session already holds the whole meeting. They appear disabled with "Not used with Claude CLI" in the AI Actions settings.

## If Claude is unavailable

If Claude cannot answer (not installed, logged out, rate limited, or it keeps crashing), NexQ switches to your **fallback provider** and shows a toast such as "Claude unavailable (not logged in) — using Groq". Pick the fallback provider in the same settings group; by default it is the last API or Ollama provider you used, or none. While Claude is unavailable, NexQ checks it again every minute and switches back when it is healthy. After a rate or usage limit it waits about 15 minutes before checking again.

## Usage indicator

The footer tooltip and AI Log header show the tokens used in the current meeting and the cost reported by Claude. Treat this as an **estimate**: with a subscription login it is included in your plan, and Claude does not expose your real remaining quota, so NexQ cannot show it. If Claude reports a rate or usage limit, NexQ warns you and uses the fallback provider.

## Privacy

- NexQ never reads your `~/.claude` folder, your credentials, or any tokens. Sign-in happens only in Claude's own tool.
- NexQ runs the unmodified `claude` program with all tools disabled, in an empty private folder. Claude cannot read your files, run commands, or browse on NexQ's behalf.
- Claude API environment variables (such as `ANTHROPIC_API_KEY`) are removed from the process, so NexQ never passes an API key to Claude. The session uses whatever account you logged in with: if that login bills an API account rather than a Pro or Max plan, Settings and the footer show "This Claude login bills an API account, not your subscription." (amber, never "Connected"). It still works, at API prices; log in with your Claude account to use your subscription instead.
- Transcript and context text you send is processed by Anthropic under your Claude account's terms, like any Claude conversation. Choose a local provider (Ollama, local Whisper) if audio and text must stay on your machine.

## Troubleshooting

| Problem | Fix |
|---------|-----|
| "Claude not found" | Install Claude Code ([setup guide](https://code.claude.com/docs/en/setup)), then click **Check connection**. |
| "Found an npm install of Claude Code" | NexQ needs the native installer (`claude.exe`), not the npm package. Install it from the [setup guide](https://code.claude.com/docs/en/setup). |
| "Bills an API account" | Your Claude login is an API (Console) login. Run `claude`, then `/login` with your Claude account. |
| "Not logged in" | Click **Open terminal to log in**, sign in, then check again. |
| Slow first answer | The first answer in a session is slower while it warms up; later answers are faster. |
| Rate or usage limit | Wait for your limit to reset, or use the fallback provider. |

See also: [Choosing AI Providers](ai-providers.md), [Configuration Guide](configuration.md), [Troubleshooting](troubleshooting.md).
