// Captures the observable shape of every built-in provider factory: identity, base
// url, the auth methods it advertises and the catalog it exposes before any refresh.
// `createProvider` hides the api map behind a closure, so the distinct `model.api`
// values stand in for it — every one of them has to dispatch in the Rust port.
import { builtinProviders } from "/Users/dev/projects/notagent-main/packages/ai/src/providers/all.ts";

const lines: string[] = [];
for (const provider of builtinProviders()) {
  const models = provider.getModels();
  lines.push(
    JSON.stringify({
      id: provider.id,
      name: provider.name,
      baseUrl: provider.baseUrl ?? null,
      headers: provider.headers ?? null,
      apiKey: provider.auth.apiKey ? { name: provider.auth.apiKey.name, hasLogin: provider.auth.apiKey.login !== undefined } : null,
      oauth: provider.auth.oauth
        ? {
            name: provider.auth.oauth.name,
            isSubscription: provider.auth.oauth.isSubscription ?? null,
            loginLabel: provider.auth.oauth.loginLabel ?? null,
          }
        : null,
      dynamic: provider.refreshModels !== undefined,
      modelCount: models.length,
      modelIds: models.map((model) => model.id),
      modelApis: [...new Set(models.map((model) => model.api))].sort(),
    }),
  );
}
process.stdout.write(`${lines.join("\n")}\n`);
