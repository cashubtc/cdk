const { existsSync } = require('fs');
const { join } = require('path');

// Try to load the native addon from the same directory
const nodePath = join(__dirname, 'index.node');

if (!existsSync(nodePath)) {
  throw new Error(
    `Native addon not found at ${nodePath}. Run: npx @napi-rs/cli build --cargo-cwd node-addon --release -o node-addon/`
  );
}

module.exports = require(nodePath);
