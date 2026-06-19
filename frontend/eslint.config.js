import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import prettier from 'eslint-config-prettier';
import globals from 'globals';
import ts from 'typescript-eslint';

export default ts.config(
	js.configs.recommended,
	...ts.configs.recommended,
	...svelte.configs['flat/recommended'],
	prettier,
	...svelte.configs['flat/prettier'],
	{
		languageOptions: {
			globals: {
				...globals.browser,
				...globals.node
			}
		}
	},
	{
		files: ['**/*.svelte'],
		languageOptions: {
			parserOptions: {
				parser: ts.parser
			}
		},
		// TypeScript + svelte-check already resolve identifiers (including `<script
		// generics>` type params, which core `no-undef` can't see). Defer to them.
		rules: {
			'no-undef': 'off'
		}
	},
	{
		// `static/` holds vendored, verbatim assets (the delta-upload worker and
		// the wasm-bindgen hash glue) — lint them as the upstream ships them.
		ignores: ['build/', '.svelte-kit/', 'package/', 'static/', 'bench/']
	}
);
