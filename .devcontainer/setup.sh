#!/usr/bin/env bash

set -e
echo "Setting up workspace..."
rustup component add rustfmt clippy
sudo apt update
sudo apt-get install -y build-essential cmake libjemalloc-dev pkg-config python3-dev protobuf-compiler
# install vg 
curl -L -O https://github.com/vgteam/vg/releases/download/v1.73.0/vg
chmod +x vg
sudo mv vg /usr/local/bin/

# install odgi
git clone --recursive https://github.com/pangenome/odgi.git
cd odgi
cmake -H. -Bbuild && cmake --build build -- -j 3
sudo mv build/bin/odgi /usr/local/bin/

alias pangraphx='/workspaces/pangraphx/target/debug/pangraphx-cli'

ln -s /usr/local/bin/pangraphx /usr/local/bin/pangraphx-cli
echo "Setup complete!"