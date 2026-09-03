# Agent logos

Official brand marks for the built-in catalog agents, bundled so the palette
works fully offline. All files are small (16–20px display size).

| File | Agent | Source |
|---|---|---|
| `claude.png` | Claude (Anthropic) | `https://claude.ai/apple-touch-icon.png` |
| `codex-color.png` | Codex (OpenAI) | OpenAI's official Codex mark via the MIT-licensed [lobehub/lobe-icons](https://github.com/lobehub/lobe-icons) collection (`packages/static-png/light/codex-color.png`) |
| `hermes.png` | Hermes Agent (Nous Research) | `https://hermes-agent.nousresearch.com/icon.png` |
| `omp-icon.svg` | Oh My Pi (omp) | `assets/icon.svg` from [can1357/oh-my-pi](https://github.com/can1357/oh-my-pi) |
| `droid.svg` | Droid (Factory) | `https://docs.factory.ai/favicon.svg` |

Logos are keyed by agent **id** in `Icons.tsx` (`AGENT_LOGO`); any other
configured agent falls back to Handover's neutral local-agent icon.

Trademarks belong to their respective owners — the marks are used here only
to identify the agents Handover hands off to.
