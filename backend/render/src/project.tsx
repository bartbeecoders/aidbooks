// Phase A placeholder project. Loaded by `cli.ts` via Revideo's
// `renderVideo({ projectFile })`. Single scene; Phase C replaces this
// with a real scene library.

import { makeProject } from '@revideo/core';

import scene from './scene.js';

export default makeProject({
  scenes: [scene],
});
