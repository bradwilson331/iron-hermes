# LoRA Local Setup Quick Installation Script for Windows PowerShell
# Run as Administrator for best results
# Usage: powershell -ExecutionPolicy Bypass -File quick_setup_script.ps1

param(
    [switch]$Full,
    [switch]$TrainingOnly,
    [switch]$ComfyOnly,
    [switch]$WebUIOnly,
    [switch]$Help
)

# Colors for output
function Write-Status { param($msg) Write-Host "[INFO] $msg" -ForegroundColor Green }
function Write-Warning { param($msg) Write-Host "[WARNING] $msg" -ForegroundColor Yellow }
function Write-Error { param($msg) Write-Host "[ERROR] $msg" -ForegroundColor Red }
function Write-Header { param($msg) Write-Host "========================================" -ForegroundColor Blue; Write-Host " $msg" -ForegroundColor Blue; Write-Host "========================================" -ForegroundColor Blue }

function Show-Help {
    Write-Host @"
LoRA Local Setup Script for Windows

Usage:
    powershell -ExecutionPolicy Bypass -File quick_setup_script.ps1 [options]

Options:
    -Full          Install everything (Kohya + ComfyUI + WebUI)
    -TrainingOnly  Install only Kohya Scripts for training
    -ComfyOnly     Install only ComfyUI for inference
    -WebUIOnly     Install only AUTOMATIC1111 WebUI
    -Help          Show this help message

Requirements:
    - Windows 10/11
    - NVIDIA GPU with recent drivers (recommended)
    - Git for Windows
    - Python 3.10-3.13

The script will:
    1. Check system requirements
    2. Install Python dependencies
    3. Set up selected AI frameworks
    4. Create startup scripts and documentation
"@
}

function Test-Requirements {
    Write-Header "Checking System Requirements"
    
    # Check Python
    try {
        $pythonVersion = python --version 2>$null
        if ($pythonVersion -match "Python 3\.(1[0-3]|[0-9])") {
            Write-Status "Python found: $pythonVersion"
            $global:PythonOK = $true
        } else {
            Write-Error "Python 3.10-3.13 required. Found: $pythonVersion"
            $global:PythonOK = $false
        }
    } catch {
        Write-Error "Python not found. Please install Python 3.10-3.13 from python.org"
        $global:PythonOK = $false
    }
    
    # Check Git
    try {
        $gitVersion = git --version 2>$null
        Write-Status "Git found: $gitVersion"
        $global:GitOK = $true
    } catch {
        Write-Error "Git not found. Please install Git for Windows"
        $global:GitOK = $false
    }
    
    # Check NVIDIA GPU
    try {
        $gpuInfo = nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits 2>$null
        if ($gpuInfo) {
            Write-Status "NVIDIA GPU detected:"
            $gpuInfo | ForEach-Object { Write-Host "  $_" }
            $global:HasCUDA = $true
            
            # Extract VRAM amount for optimization
            $vramMB = [int]($gpuInfo -split ',')[1].Trim()
            $global:VRAM = $vramMB
        } else {
            throw "No NVIDIA GPU found"
        }
    } catch {
        Write-Warning "NVIDIA GPU not detected. Will install CPU-only version."
        $global:HasCUDA = $false
        $global:VRAM = 0
    }
    
    return ($global:PythonOK -and $global:GitOK)
}

function Install-PythonEnvironment {
    Write-Header "Setting up Python Environment"
    
    # Create virtual environment
    Write-Status "Creating virtual environment..."
    python -m venv lora_env
    
    # Activate environment
    & ".\lora_env\Scripts\Activate.ps1"
    
    # Upgrade pip
    Write-Status "Upgrading pip..."
    python -m pip install --upgrade pip
    
    # Install PyTorch
    if ($global:HasCUDA) {
        Write-Status "Installing PyTorch with CUDA support..."
        pip install torch==2.6.0 torchvision==0.21.0 torchaudio --index-url https://download.pytorch.org/whl/cu124
    } else {
        Write-Status "Installing CPU-only PyTorch..."
        pip install torch torchvision torchaudio
    }
    
    # Install core dependencies
    Write-Status "Installing core AI libraries..."
    $corePackages = @(
        "accelerate>=0.26.0",
        "transformers>=4.36.0", 
        "diffusers>=0.24.0",
        "peft>=0.7.0",
        "safetensors>=0.4.0",
        "opencv-python",
        "Pillow",
        "numpy",
        "scipy",
        "tqdm",
        "wandb"
    )
    
    foreach ($package in $corePackages) {
        pip install $package
    }
    
    # Install xformers if CUDA available
    if ($global:HasCUDA) {
        Write-Status "Installing xformers for memory optimization..."
        pip install xformers --index-url https://download.pytorch.org/whl/cu124
    }
    
    # Install optional packages
    pip install matplotlib jupyter ipywidgets
}

function Install-KohyaScripts {
    Write-Header "Setting up Kohya Scripts"
    
    if (-not (Test-Path "kohya-scripts")) {
        git clone https://github.com/kohya-ss/sd-scripts.git kohya-scripts
    }
    
    Set-Location kohya-scripts
    & "..\lora_env\Scripts\Activate.ps1"
    
    # Install Kohya requirements
    pip install -r requirements.txt
    
    # Configure accelerate
    Write-Status "Configuring accelerate..."
    $accelerateConfig = @"
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
"@
    
    $configDir = "$env:USERPROFILE\.cache\huggingface\accelerate"
    if (-not (Test-Path $configDir)) {
        New-Item -ItemType Directory -Path $configDir -Force
    }
    $accelerateConfig | Out-File -FilePath "$configDir\default_config.yaml" -Encoding UTF8
    
    Set-Location ..
}

function Install-ComfyUI {
    Write-Header "Setting up ComfyUI"
    
    if (-not (Test-Path "ComfyUI")) {
        git clone https://github.com/comfyanonymous/ComfyUI.git
    }
    
    Set-Location ComfyUI
    & "..\lora_env\Scripts\Activate.ps1"
    
    # Install requirements
    pip install -r requirements.txt
    
    # Create model directories
    $modelDirs = @("checkpoints", "lora", "vae", "embeddings", "controlnet")
    foreach ($dir in $modelDirs) {
        $path = "models\$dir"
        if (-not (Test-Path $path)) {
            New-Item -ItemType Directory -Path $path -Force
        }
    }
    
    # Download basic VAE
    $vaeFile = "models\vae\vae-ft-mse-840000-ema-pruned.safetensors"
    if (-not (Test-Path $vaeFile)) {
        Write-Status "Downloading basic VAE..."
        $vaeUrl = "https://huggingface.co/stabilityai/sd-vae-ft-mse-original/resolve/main/vae-ft-mse-840000-ema-pruned.safetensors"
        Invoke-WebRequest -Uri $vaeUrl -OutFile $vaeFile
    }
    
    Set-Location ..
}

function Install-WebUI {
    Write-Header "Setting up AUTOMATIC1111 WebUI"
    
    if (-not (Test-Path "stable-diffusion-webui")) {
        git clone https://github.com/AUTOMATIC1111/stable-diffusion-webui.git
    }
    
    Set-Location stable-diffusion-webui
    
    # Create optimized launch script
    $launchScript = @"
@echo off
call ..\lora_env\Scripts\activate.bat

REM Auto-detect optimization flags
if exist "%PROGRAMFILES%\NVIDIA Corporation\NVSMI\nvidia-smi.exe" (
    "%PROGRAMFILES%\NVIDIA Corporation\NVSMI\nvidia-smi.exe" >nul 2>&1
    if errorlevel 0 (
        REM NVIDIA GPU detected - choose flags based on VRAM
        if $($global:VRAM) LSS 8000 (
            set FLAGS=--xformers --medvram --opt-split-attention
        ) else if $($global:VRAM) LSS 12000 (
            set FLAGS=--xformers --opt-sdp-attention  
        ) else (
            set FLAGS=--xformers --opt-sdp-no-mem-attention --opt-channelslast
        )
    ) else (
        set FLAGS=--use-cpu all --precision autocast
    )
) else (
    set FLAGS=--use-cpu all --precision autocast
)

python launch.py %FLAGS% --listen --enable-insecure-extension-access
pause
"@
    
    $launchScript | Out-File -FilePath "launch_optimized.bat" -Encoding ASCII
    
    Set-Location ..
}

function Create-TrainingExample {
    Write-Header "Creating Example Training Script"
    
    $trainingScript = @"
@echo off
REM Example LoRA training script
REM Modify paths and parameters as needed

call lora_env\Scripts\activate.bat
cd kohya-scripts

REM Configuration
set MODEL_NAME=runwayml/stable-diffusion-v1-5
set INSTANCE_DIR=.\training_data\images
set CLASS_DIR=.\training_data\class_images  
set OUTPUT_DIR=.\output\lora_model

REM Create directories
if not exist "%INSTANCE_DIR%" mkdir "%INSTANCE_DIR%"
if not exist "%CLASS_DIR%" mkdir "%CLASS_DIR%"
if not exist "%OUTPUT_DIR%" mkdir "%OUTPUT_DIR%"

REM Training command
accelerate launch train_network.py ^
    --pretrained_model_name_or_path="%MODEL_NAME%" ^
    --train_data_dir="%INSTANCE_DIR%" ^
    --resolution=512 ^
    --output_dir="%OUTPUT_DIR%" ^
    --logging_dir="./logs" ^
    --network_alpha=128 ^
    --save_model_as=safetensors ^
    --network_module=networks.lora ^
    --network_dim=128 ^
    --output_name="my_lora" ^
    --lr_scheduler="cosine_with_restarts" ^
    --learning_rate=1e-4 ^
    --unet_lr=1e-4 ^
    --text_encoder_lr=5e-5 ^
    --train_batch_size=1 ^
    --max_train_steps=1500 ^
    --mixed_precision="fp16" ^
    --save_precision="fp16" ^
    --cache_latents ^
    --cache_latents_to_disk ^
    --prior_loss_weight=1.0 ^
    --max_data_loader_n_workers=0 ^
    --persistent_data_loader_workers ^
    --bucket_no_upscale ^
    --random_crop

pause
"@
    
    $trainingScript | Out-File -FilePath "train_lora_example.bat" -Encoding ASCII
}

function Create-StartupScripts {
    Write-Header "Creating Startup Scripts"
    
    # ComfyUI launcher
    $comfyScript = @"
@echo off
call lora_env\Scripts\activate.bat
cd ComfyUI

REM Auto-detect optimization flags  
if $($global:VRAM) LSS 8000 (
    set FLAGS=--lowvram
) else if $($global:VRAM) LSS 12000 (
    set FLAGS=--medvram
) else (
    set FLAGS=
)

if not $($global:HasCUDA) (
    set FLAGS=--cpu
)

python main.py %FLAGS% --listen --preview-method taesd
pause
"@
    
    # WebUI launcher
    $webuiScript = @"
@echo off
cd stable-diffusion-webui
call launch_optimized.bat
"@
    
    $comfyScript | Out-File -FilePath "start_comfyui.bat" -Encoding ASCII
    $webuiScript | Out-File -FilePath "start_webui.bat" -Encoding ASCII
}

function Create-Documentation {
    Write-Header "Creating Documentation"
    
    $readme = @"
# LoRA Local Setup - Quick Start Guide (Windows)

## What was installed:

1. **Python Environment**: `lora_env\` - Virtual environment with all dependencies
2. **Kohya Scripts**: `kohya-scripts\` - Professional LoRA training framework  
3. **ComfyUI**: `ComfyUI\` - Advanced node-based interface
4. **WebUI**: `stable-diffusion-webui\` - User-friendly web interface

## Quick Start:

### Start ComfyUI (Recommended):
Double-click: `start_comfyui.bat`
Then visit http://localhost:8188

### Start WebUI:
Double-click: `start_webui.bat` 
Then visit http://localhost:7860

### Train a LoRA:
1. Place training images in `training_data\images\`
2. Modify `train_lora_example.bat` parameters
3. Run: `train_lora_example.bat`

## System Specs Detected:

- GPU VRAM: $($global:VRAM)MB
- CUDA Support: $($global:HasCUDA)
- Optimization Level: $(if($global:VRAM -gt 12000){"High"}elseif($global:VRAM -gt 8000){"Medium"}else{"Basic"})

## Next Steps:

1. Download base models to appropriate folders:
   - Checkpoints: `ComfyUI\models\checkpoints\` or `stable-diffusion-webui\models\Stable-diffusion\`
   - LoRAs: `ComfyUI\models\lora\` or `stable-diffusion-webui\models\Lora\`

2. Recommended base models:
   - Stable Diffusion 1.5: https://huggingface.co/runwayml/stable-diffusion-v1-5
   - SDXL: https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0

3. For training:
   - Prepare high-quality dataset (20-100 images minimum)
   - Write descriptive captions for each image
   - Adjust training parameters in example script

## Troubleshooting:

- **Out of Memory**: Try lower resolution or batch size
- **Slow Performance**: Ensure latest NVIDIA drivers installed
- **Scripts Won't Run**: Check that Python and Git are in PATH

For detailed guide, see: lora_local_setup_guide.md

Created: $(Get-Date)
"@
    
    $readme | Out-File -FilePath "README_SETUP.txt" -Encoding UTF8
}

function Main {
    if ($Help) {
        Show-Help
        return
    }
    
    Write-Header "LoRA Local Setup Installer for Windows"
    Write-Status "This script will install everything needed for local LoRA training and inference"
    
    # Check requirements
    if (-not (Test-Requirements)) {
        Write-Error "System requirements not met. Please install missing components and try again."
        return
    }
    
    # Determine installation mode
    $mode = "prompt"
    if ($Full) { $mode = "full" }
    elseif ($TrainingOnly) { $mode = "training" }
    elseif ($ComfyOnly) { $mode = "comfy" }  
    elseif ($WebUIOnly) { $mode = "webui" }
    
    if ($mode -eq "prompt") {
        Write-Host "Select installation mode:"
        Write-Host "1) Complete setup (Kohya + ComfyUI + WebUI)"
        Write-Host "2) Training only (Kohya Scripts)"
        Write-Host "3) ComfyUI only"
        Write-Host "4) WebUI only"
        $choice = Read-Host "Choice (1-4)"
        
        switch ($choice) {
            "1" { $mode = "full" }
            "2" { $mode = "training" }
            "3" { $mode = "comfy" }
            "4" { $mode = "webui" }
            default { Write-Error "Invalid choice"; return }
        }
    }
    
    # Install Python environment
    Install-PythonEnvironment
    
    # Install components based on mode
    switch ($mode) {
        "full" {
            Install-KohyaScripts
            Install-ComfyUI
            Install-WebUI
            Create-TrainingExample
        }
        "training" {
            Install-KohyaScripts
            Create-TrainingExample
        }
        "comfy" {
            Install-ComfyUI
        }
        "webui" {
            Install-WebUI
        }
    }
    
    Create-StartupScripts
    Create-Documentation
    
    Write-Header "Installation Complete!"
    Write-Status "Check README_SETUP.txt for next steps"
    
    if ($mode -in @("full", "comfy")) {
        Write-Status "Start ComfyUI: Double-click start_comfyui.bat"
    }
    if ($mode -in @("full", "webui")) {
        Write-Status "Start WebUI: Double-click start_webui.bat"
    }
    if ($mode -in @("full", "training")) {
        Write-Status "Train LoRA: Run train_lora_example.bat (after preparing data)"
    }
}

# Execute main function
Main