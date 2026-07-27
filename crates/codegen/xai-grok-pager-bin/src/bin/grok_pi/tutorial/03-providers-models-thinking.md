# Providers, Models & Thinking

Pi is a multi-provider agent core. Depending on your Pi installation, its model
registry can use providers such as Anthropic, OpenAI, Google, Bedrock, Azure,
Groq, xAI and OpenRouter.

- `/model` opens the native Pager picker over Pi's available model catalog.
- `/effort` changes the thinking level supported by the selected model.
- Startup flags include `--provider`, `--model`, `--models` and `--thinking`.
- Default-on `/login` and `/logout` use Pi's provider authentication through the
  Remote TUI host when that compatibility layer is enabled.
- `~/.pi/agent/models.json` can describe OpenAI-compatible local endpoints such
  as Ollama, LM Studio or vLLM.
- An extension can call `registerProvider` to add OAuth, dynamic model discovery
  or a completely custom streaming API.

Pager renders the selector; Pi still owns credentials, provider behavior and
model requests.
