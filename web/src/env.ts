import { defineEnvVars } from '@sveltejs/kit/env';

export const variables = defineEnvVars({
  PUBLIC_DEV_MODE: { public: true, schema: (input) => input ?? '' }
});
