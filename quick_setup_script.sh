#!/bin/bash

# LoRA Local Setup Quick Installation Script
# Supports Ubuntu/Debian, CentOS/RHEL, and macOS
# Run with: bash quick_setup_script.sh

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Function to print colored output
print_status() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_header() {
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE} $1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# Detect OS
detect_os() {
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        if [ -f /etc/debian_version ]; then
            OS="debian"
        elif [ -f /etc/redhat-release ]; then
            OS="rhel"
        else
            OS="linux"
        fi
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        OS="macos"
    else
        print_error "Unsupported operating system: $OSTYPE"
        exit 1
    fi
    print_status "Detected OS: $OS"
}

# Check for CUDA
check_cuda() {
    if command -v nvidia-smi &> /dev/null; then
        print_status "NVIDIA GPU detected:"
        nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits
        HAS_CUDA=true
    else
        print_warning "NVIDIA GPU or drivers not detected. Will install CPU-only version."
        HAS_CUDA=false
    fi
}

# Install system dependencies
install_system_deps() {
    print_header "Installing System Dependencies"
    
    case $OS in
        debian)
            sudo apt update
            sudo apt install -y python3 python3-pip python3-venv git wget curl build-essential
            if [ "$HAS_CUDA" = true ]; then
                # Check if CUDA toolkit is installed
                if ! dpkg -l | grep -q cuda-toolkit; then
                    print_warning "CUDA toolkit not found. Please install it manually:"
                    print_warning "https://developer.nvidia.com/cuda-downloads"
                fi
            fi
            ;;
        rhel)
            sudo dnf groupinstall -y "Development Tools"
            sudo dnf install -y python3 python3-pip git wget curl
            ;;
        macos)
            # Check for Homebrew
            if ! command -v brew &> /dev/null; then
                print_status "Installing Homebrew..."
                /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
            fi
            brew install python@3.11 git wget
            ;;
    esac
}

# Install Python packages
setup_python_env() {
    print_header "Setting up Python Environment"
    
    # Create virtual environment
    python3 -m venv lora_env
    source lora_env/bin/activate
    
    # Upgrade pip
    pip install --upgrade pip
    
    # Install PyTorch based on system
    if [ "$HAS_CUDA" = true ]; then
        print_status "Installing PyTorch with CUDA support..."
        pip install torch==2.6.0 torchvision==0.21.0 torchaudio --index-url https://download.pytorch.org/whl/cu124
    elif [ "$OS" = "macos" ]; then
        print_status "Installing PyTorch for Apple Silicon..."
        pip install torch torchvision torchaudio
    else
        print_status "Installing CPU-only PyTorch..."
        pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cpu
    fi
    
    # Install core dependencies
    print_status "Installing core AI libraries..."
    pip install accelerate>=0.26.0 transformers>=4.36.0 diffusers>=0.24.0 peft>=0.7.0
    pip install safetensors>=0.4.0 opencv-python Pillow numpy scipy tqdm wandb
    
    # Install xformers if CUDA available
    if [ "$HAS_CUDA" = true ]; then
        print_status "Installing xformers for memory optimization..."
        pip install xformers --index-url https://download.pytorch.org/whl/cu124
    fi
    
    # Install optional but useful packages
    pip install matplotlib jupyter ipywidgets
}

# Setup Kohya Scripts
setup_kohya() {
    print_header "Setting up Kohya Scripts"
    
    if [ ! -d "kohya-scripts" ]; then
        git clone https://github.com/kohya-ss/sd-scripts.git kohya-scripts
    fi
    
    cd kohya-scripts
    source ../lora_env/bin/activate
    
    # Install Kohya requirements
    pip install -r requirements.txt
    
    # Configure accelerate
    print_status "Configuring accelerate..."
    cat > accelerate_config.yaml << EOF
compute_environment: LOCAL_MACHINE
distributed_type: 'NO'
downcast_bf16: 'no'
gpu_ids: all
machine_rank: 0
main_training_function: main
mixed_precision: fp16
num_machines: 1
num_processes: 1
rdzv_backend: static
same_network: true
tpu_env: []
tpu_use_cluster: false
tpu_use_sudo: false
use_cpu: false
EOF
    
    # Move config to accelerate directory
    mkdir -p ~/.cache/huggingface/accelerate
    cp accelerate_config.yaml ~/.cache/huggingface/accelerate/default_config.yaml
    
    cd ..
}

# Setup ComfyUI
setup_comfyui() {
    print_header "Setting up ComfyUI"
    
    if [ ! -d "ComfyUI" ]; then
        git clone https://github.com/comfyanonymous/ComfyUI.git
    fi
    
    cd ComfyUI
    source ../lora_env/bin/activate
    
    # Install ComfyUI requirements
    pip install -r requirements.txt
    
    # Create model directories
    mkdir -p models/{checkpoints,lora,vae,embeddings,controlnet}
    
    # Download basic VAE
    if [ ! -f "models/vae/vae-ft-mse-840000-ema-pruned.safetensors" ]; then
        print_status "Downloading basic VAE..."
        wget -O models/vae/vae-ft-mse-840000-ema-pruned.safetensors \
            "https://huggingface.co/stabilityai/sd-vae-ft-mse-original/resolve/main/vae-ft-mse-840000-ema-pruned.safetensors"
    fi
    
    cd ..
}

# Setup AUTOMATIC1111 WebUI
setup_automatic1111() {
    print_header "Setting up AUTOMATIC1111 WebUI"
    
    if [ ! -d "stable-diffusion-webui" ]; then
        git clone https://github.com/AUTOMATIC1111/stable-diffusion-webui.git
    fi
    
    cd stable-diffusion-webui
    
    # Create launch script
    cat > launch_optimized.sh << 'EOF'
#!/bin/bash
source ../lora_env/bin/activate

# Determine optimal flags based on system
if nvidia-smi &> /dev/null; then
    # NVIDIA GPU detected
    VRAM=$(nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits | head -1)
    if [ "$VRAM" -lt 8000 ]; then
        FLAGS="--xformers --medvram --opt-split-attention"
    elif [ "$VRAM" -lt 12000 ]; then
        FLAGS="--xformers --opt-sdp-attention"
    else
        FLAGS="--xformers --opt-sdp-no-mem-attention --opt-channelslast"
    fi
else
    # CPU or non-NVIDIA
    FLAGS="--use-cpu all --precision autocast"
fi

python launch.py $FLAGS --listen --enable-insecure-extension-access
EOF
    
    chmod +x launch_optimized.sh
    cd ..
}

# Create example training script
create_training_example() {
    print_header "Creating Example Training Script"
    
    cat > train_lora_example.sh << 'EOF'
#!/bin/bash

# Example LoRA training script
# Modify paths and parameters as needed

source lora_env/bin/activate
cd kohya-scripts

# Configuration
MODEL_NAME="runwayml/stable-diffusion-v1-5"
INSTANCE_DIR="./training_data/images"
CLASS_DIR="./training_data/class_images"
OUTPUT_DIR="./output/lora_model"

# Create directories
mkdir -p "$INSTANCE_DIR" "$CLASS_DIR" "$OUTPUT_DIR"

# Training command
accelerate launch train_network.py \
    --pretrained_model_name_or_path="$MODEL_NAME" \
    --train_data_dir="$INSTANCE_DIR" \
    --resolution=512 \
    --output_dir="$OUTPUT_DIR" \
    --logging_dir="./logs" \
    --network_alpha=128 \
    --save_model_as=safetensors \
    --network_module=networks.lora \
    --network_dim=128 \
    --output_name="my_lora" \
    --lr_scheduler="cosine_with_restarts" \
    --learning_rate=1e-4 \
    --unet_lr=1e-4 \
    --text_encoder_lr=5e-5 \
    --train_batch_size=1 \
    --max_train_steps=1500 \
    --mixed_precision="fp16" \
    --save_precision="fp16" \
    --cache_latents \
    --cache_latents_to_disk \
    --prior_loss_weight=1.0 \
    --max_data_loader_n_workers=0 \
    --persistent_data_loader_workers \
    --bucket_no_upscale \
    --random_crop
EOF
    
    chmod +x train_lora_example.sh
}

# Create startup scripts
create_startup_scripts() {
    print_header "Creating Startup Scripts"
    
    # ComfyUI launcher
    cat > start_comfyui.sh << 'EOF'
#!/bin/bash
source lora_env/bin/activate
cd ComfyUI

# Auto-detect optimization flags
if nvidia-smi &> /dev/null; then
    VRAM=$(nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits | head -1)
    if [ "$VRAM" -lt 8000 ]; then
        FLAGS="--lowvram"
    elif [ "$VRAM" -lt 12000 ]; then
        FLAGS="--medvram"
    else
        FLAGS=""
    fi
else
    FLAGS="--cpu"
fi

python main.py $FLAGS --listen --preview-method taesd
EOF
    
    # WebUI launcher  
    cat > start_webui.sh << 'EOF'
#!/bin/bash
cd stable-diffusion-webui
./launch_optimized.sh
EOF
    
    chmod +x start_comfyui.sh start_webui.sh
}

# Create README
create_readme() {
    print_header "Creating Documentation"
    
    cat > README_SETUP.md << 'EOF'
# LoRA Local Setup - Quick Start Guide

## What was installed:

1. **Python Environment**: `lora_env/` - Virtual environment with all dependencies
2. **Kohya Scripts**: `kohya-scripts/` - Professional LoRA training framework
3. **ComfyUI**: `ComfyUI/` - Advanced node-based interface
4. **WebUI**: `stable-diffusion-webui/` - User-friendly web interface

## Quick Start:

### Start ComfyUI (Recommended):
```bash
./start_comfyui.sh
```
Then visit http://localhost:8188

### Start WebUI:
```bash
./start_webui.sh
```
Then visit http://localhost:7860

### Train a LoRA:
1. Place training images in `training_data/images/`
2. Modify `train_lora_example.sh` parameters
3. Run: `./train_lora_example.sh`

## Next Steps:

1. Download base models to appropriate folders:
   - Checkpoints: `ComfyUI/models/checkpoints/` or `stable-diffusion-webui/models/Stable-diffusion/`
   - LoRAs: `ComfyUI/models/lora/` or `stable-diffusion-webui/models/Lora/`

2. Recommended base models:
   - Stable Diffusion 1.5: https://huggingface.co/runwayml/stable-diffusion-v1-5
   - SDXL: https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0

3. For training:
   - Prepare high-quality dataset (20-100 images minimum)
   - Write descriptive captions for each image
   - Adjust training parameters in example script

## Troubleshooting:

- **Out of Memory**: Use `--medvram` or `--lowvram` flags
- **Slow Performance**: Ensure CUDA/appropriate GPU drivers installed
- **Import Errors**: Activate virtual environment: `source lora_env/bin/activate`

For detailed guide, see: lora_local_setup_guide.md
EOF
}

# Main installation function
main() {
    print_header "LoRA Local Setup Installer"
    print_status "This script will install everything needed for local LoRA training and inference"
    
    # Ask for user confirmation
    read -p "Continue with installation? (y/N): " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        print_status "Installation cancelled."
        exit 0
    fi
    
    # Ask which components to install
    echo "Select components to install:"
    echo "1) Complete setup (Kohya + ComfyUI + WebUI)"
    echo "2) Kohya Scripts only (training)"
    echo "3) ComfyUI only (inference)"
    echo "4) WebUI only (inference)"
    read -p "Choice (1-4): " -n 1 -r CHOICE
    echo
    
    # Run installation steps
    detect_os
    check_cuda
    install_system_deps
    setup_python_env
    
    case $CHOICE in
        1)
            setup_kohya
            setup_comfyui
            setup_automatic1111
            create_training_example
            ;;
        2)
            setup_kohya
            create_training_example
            ;;
        3)
            setup_comfyui
            ;;
        4)
            setup_automatic1111
            ;;
        *)
            print_error "Invalid choice"
            exit 1
            ;;
    esac
    
    create_startup_scripts
    create_readme
    
    print_header "Installation Complete!"
    print_status "Check README_SETUP.md for next steps"
    print_status "Activate environment with: source lora_env/bin/activate"
    
    if [ "$CHOICE" = "1" ] || [ "$CHOICE" = "3" ]; then
        print_status "Start ComfyUI with: ./start_comfyui.sh"
    fi
    if [ "$CHOICE" = "1" ] || [ "$CHOICE" = "4" ]; then
        print_status "Start WebUI with: ./start_webui.sh"
    fi
    if [ "$CHOICE" = "1" ] || [ "$CHOICE" = "2" ]; then
        print_status "Train LoRA with: ./train_lora_example.sh (after preparing data)"
    fi
}

# Run main function
main "$@"
EOF