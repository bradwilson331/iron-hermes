# LoRA Implementation Guide: Step-by-Step Setup for Maximum Realism

## Quick Start Setup

### System Requirements Verification
```bash
# Check GPU
nvidia-smi

# Verify VRAM (minimum 12GB recommended for SDXL)
nvidia-smi --query-gpu=memory.total --format=csv,noheader,nounits

# Check available disk space (minimum 100GB recommended)
df -h
```

### 1. Environment Setup (Windows/Linux)

#### Windows Installation (Recommended: uv method)
```powershell
# Install Python 3.10+ and Git first
# Clone Kohya_ss
git clone https://github.com/bmaltais/kohya_ss.git
cd kohya_ss

# Install dependencies
pip install uv
uv venv
.venv\Scripts\activate  # Windows
pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu124
pip install -r requirements.txt

# Optional but recommended: xFormers for speed
pip install xformers --index-url https://download.pytorch.org/whl/cu124
```

#### Linux Installation
```bash
git clone https://github.com/bmaltais/kohya_ss.git
cd kohya_ss
python -m venv venv
source venv/bin/activate
pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu124
pip install -r requirements.txt
pip install xformers --index-url https://download.pytorch.org/whl/cu124
```

### 2. Configuration Setup

#### Create config.toml for default paths
```toml
# config.toml - Place in kohya_ss root directory
[general]
model_dir = "/path/to/your/base/models"
lora_model_dir = "/path/to/your/lora/outputs"
output_dir = "/path/to/training/outputs"
dataset_dir = "/path/to/your/datasets"
vae_dir = "/path/to/vae/models"

[training]
# Default training settings
mixed_precision = "fp16"
save_precision = "fp16"
cache_latents = true
cache_latents_to_disk = true
```

## Dataset Preparation Workflow

### 1. Image Collection and Organization
```
dataset_folder/
├── concept_name/
│   ├── img/
│   │   ├── 001.jpg
│   │   ├── 002.jpg
│   │   └── ...
│   └── captions/
│       ├── 001.txt
│       ├── 002.txt
│       └── ...
└── regularization/ (for DreamBooth)
    └── class_images/
```

### 2. Automated Tagging Script
```python
# auto_tag.py - Run in kohya_ss directory
import subprocess
import os

def auto_tag_images(image_folder, threshold=0.35):
    """
    Automatically tag images using WD14 tagger
    """
    cmd = [
        "python", "finetune/tag_images_by_wd14_tagger.py",
        "--batch_size", "4",
        "--thresh", str(threshold),
        "--caption_extension", ".txt",
        image_folder
    ]
    
    result = subprocess.run(cmd, capture_output=True, text=True)
    print(f"Tagging completed: {result.stdout}")
    return result.returncode == 0

# Usage
image_path = "/path/to/your/dataset/concept_name/img"
auto_tag_images(image_path)
```

### 3. Caption Enhancement Script
```python
# enhance_captions.py
import os
import glob

def enhance_captions(caption_folder, trigger_word, style_tags):
    """
    Enhance existing captions with trigger words and style tags
    """
    caption_files = glob.glob(os.path.join(caption_folder, "*.txt"))
    
    for caption_file in caption_files:
        with open(caption_file, 'r', encoding='utf-8') as f:
            original_caption = f.read().strip()
        
        # Add trigger word at the beginning
        enhanced_caption = f"{trigger_word}, {original_caption}"
        
        # Add style tags
        if style_tags:
            enhanced_caption += f", {', '.join(style_tags)}"
        
        with open(caption_file, 'w', encoding='utf-8') as f:
            f.write(enhanced_caption)
    
    print(f"Enhanced {len(caption_files)} captions")

# Usage example
enhance_captions(
    "/path/to/captions",
    trigger_word="mycharacter",
    style_tags=["photorealistic", "high quality", "detailed"]
)
```

## Training Configuration Templates

### 1. Maximum Quality Configuration (SDXL - High-End Hardware)
```json
{
  "model_arguments": {
    "pretrained_model_name_or_path": "stabilityai/stable-diffusion-xl-base-1.0",
    "vae": "madebyollin/sdxl-vae-fp16-fix"
  },
  "dataset_arguments": {
    "dataset_config": "dataset_config.toml",
    "cache_latents": true,
    "cache_latents_to_disk": true
  },
  "training_arguments": {
    "output_dir": "./output",
    "logging_dir": "./logs",
    "resolution": 1024,
    "train_batch_size": 2,
    "gradient_accumulation_steps": 2,
    "learning_rate": 5e-5,
    "lr_scheduler": "cosine_with_restarts",
    "lr_warmup_steps": 100,
    "max_train_steps": 3000,
    "mixed_precision": "fp16",
    "save_precision": "fp16",
    "seed": 42
  },
  "lora_arguments": {
    "network_module": "networks.lora",
    "network_dim": 128,
    "network_alpha": 64,
    "network_train_unet_only": true,
    "network_train_text_encoder_only": false
  },
  "optimizer_arguments": {
    "optimizer_type": "AdamW8bit",
    "beta1": 0.9,
    "beta2": 0.999,
    "weight_decay": 0.01,
    "epsilon": 1e-8
  },
  "additional_arguments": {
    "gradient_checkpointing": true,
    "xformers": true,
    "noise_offset": 0.1,
    "adaptive_noise_scale": 0.00357,
    "multires_noise_iterations": 10,
    "multires_noise_discount": 0.1
  }
}
```

### 2. Balanced Performance Configuration (SDXL - Mid-Range Hardware)
```json
{
  "model_arguments": {
    "pretrained_model_name_or_path": "stabilityai/stable-diffusion-xl-base-1.0",
    "vae": "madebyollin/sdxl-vae-fp16-fix"
  },
  "dataset_arguments": {
    "dataset_config": "dataset_config.toml",
    "cache_latents": true,
    "cache_latents_to_disk": true
  },
  "training_arguments": {
    "output_dir": "./output",
    "logging_dir": "./logs",
    "resolution": 1024,
    "train_batch_size": 1,
    "gradient_accumulation_steps": 4,
    "learning_rate": 1e-4,
    "lr_scheduler": "cosine",
    "lr_warmup_steps": 50,
    "max_train_steps": 2000,
    "mixed_precision": "fp16",
    "save_precision": "fp16"
  },
  "lora_arguments": {
    "network_module": "networks.lora",
    "network_dim": 64,
    "network_alpha": 32,
    "network_train_unet_only": true
  },
  "optimizer_arguments": {
    "optimizer_type": "AdamW8bit"
  },
  "additional_arguments": {
    "gradient_checkpointing": true,
    "xformers": true,
    "noise_offset": 0.05
  }
}
```

### 3. Fast Iteration Configuration (SD 1.5 - Budget Hardware)
```json
{
  "model_arguments": {
    "pretrained_model_name_or_path": "runwayml/stable-diffusion-v1-5"
  },
  "training_arguments": {
    "resolution": 512,
    "train_batch_size": 2,
    "gradient_accumulation_steps": 2,
    "learning_rate": 1e-4,
    "max_train_steps": 1500,
    "mixed_precision": "fp16"
  },
  "lora_arguments": {
    "network_dim": 32,
    "network_alpha": 16
  },
  "additional_arguments": {
    "gradient_checkpointing": true,
    "xformers": true
  }
}
```

## Advanced Training Scripts

### 1. Automated Training Pipeline
```python
# train_pipeline.py
import os
import subprocess
import json
import time
from pathlib import Path

class LoRATrainer:
    def __init__(self, config_path):
        with open(config_path, 'r') as f:
            self.config = json.load(f)
        
    def prepare_dataset_config(self, dataset_path, repeats=10):
        """Generate dataset configuration file"""
        config_content = f"""
[[datasets]]
[[datasets.subsets]]
image_dir = '{dataset_path}/img'
caption_extension = '.txt'
num_repeats = {repeats}
shuffle_caption = true
keep_tokens = 1
        """
        
        config_path = "dataset_config.toml"
        with open(config_path, 'w') as f:
            f.write(config_content.strip())
        
        return config_path
    
    def build_training_command(self):
        """Build the training command from configuration"""
        cmd = ["python", "sd-scripts/train_network.py"]
        
        # Add all configuration parameters
        for section, params in self.config.items():
            for key, value in params.items():
                if isinstance(value, bool):
                    if value:
                        cmd.append(f"--{key}")
                else:
                    cmd.extend([f"--{key}", str(value)])
        
        return cmd
    
    def train(self, dataset_path):
        """Execute training"""
        print("Preparing dataset configuration...")
        self.prepare_dataset_config(dataset_path)
        
        print("Starting training...")
        cmd = self.build_training_command()
        
        # Log the command
        print("Training command:", " ".join(cmd))
        
        # Execute training
        process = subprocess.Popen(
            cmd, 
            stdout=subprocess.PIPE, 
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
            universal_newlines=True
        )
        
        # Real-time output
        for line in process.stdout:
            print(line.strip())
        
        process.wait()
        return process.returncode == 0

# Usage
trainer = LoRATrainer("config.json")
success = trainer.train("/path/to/your/dataset")
```

### 2. Quality Assessment Script
```python
# assess_quality.py
import torch
import clip
from PIL import Image
import os
import numpy as np
from diffusers import AutoPipelineForText2Image

class QualityAssessment:
    def __init__(self, model_path, lora_path):
        self.device = "cuda" if torch.cuda.is_available() else "cpu"
        
        # Load CLIP for evaluation
        self.clip_model, self.clip_preprocess = clip.load("ViT-B/32", device=self.device)
        
        # Load diffusion model with LoRA
        self.pipeline = AutoPipelineForText2Image.from_pretrained(
            model_path, 
            torch_dtype=torch.float16 if self.device == "cuda" else torch.float32
        ).to(self.device)
        
        if lora_path:
            self.pipeline.load_lora_weights(lora_path)
    
    def generate_test_images(self, prompts, num_samples=4):
        """Generate test images for evaluation"""
        results = {}
        
        for prompt in prompts:
            images = []
            for _ in range(num_samples):
                image = self.pipeline(
                    prompt, 
                    num_inference_steps=20, 
                    guidance_scale=7.5,
                    generator=torch.Generator().manual_seed(torch.randint(0, 2**32, (1,)).item())
                ).images[0]
                images.append(image)
            
            results[prompt] = images
        
        return results
    
    def calculate_clip_score(self, images, prompt):
        """Calculate CLIP score for prompt-image alignment"""
        scores = []
        
        text = clip.tokenize([prompt]).to(self.device)
        text_features = self.clip_model.encode_text(text)
        
        for image in images:
            image_input = self.clip_preprocess(image).unsqueeze(0).to(self.device)
            image_features = self.clip_model.encode_image(image_input)
            
            # Normalize features
            image_features = image_features / image_features.norm(dim=-1, keepdim=True)
            text_features = text_features / text_features.norm(dim=-1, keepdim=True)
            
            # Calculate cosine similarity
            score = torch.cosine_similarity(image_features, text_features).item()
            scores.append(score)
        
        return np.mean(scores), np.std(scores)
    
    def assess_quality(self, test_prompts):
        """Comprehensive quality assessment"""
        print("Generating test images...")
        results = self.generate_test_images(test_prompts)
        
        assessment = {}
        for prompt, images in results.items():
            clip_mean, clip_std = self.calculate_clip_score(images, prompt)
            
            assessment[prompt] = {
                'clip_score_mean': clip_mean,
                'clip_score_std': clip_std,
                'num_images': len(images)
            }
            
            print(f"Prompt: {prompt}")
            print(f"  CLIP Score: {clip_mean:.3f} ± {clip_std:.3f}")
        
        return assessment

# Usage example
test_prompts = [
    "mycharacter smiling in a sunny park",
    "mycharacter wearing a red jacket, professional photo",
    "close-up portrait of mycharacter with natural lighting"
]

assessor = QualityAssessment(
    "stabilityai/stable-diffusion-xl-base-1.0",
    "/path/to/your/lora.safetensors"
)

results = assessor.assess_quality(test_prompts)
```

## Monitoring and Optimization

### 1. Training Monitor Script
```python
# monitor_training.py
import psutil
import nvidia_ml_py3 as nvml
import time
import matplotlib.pyplot as plt
from collections import deque
import threading

class TrainingMonitor:
    def __init__(self, log_interval=10):
        self.log_interval = log_interval
        self.running = False
        
        # Initialize NVIDIA ML
        nvml.nvmlInit()
        self.device_count = nvml.nvmlDeviceGetCount()
        
        # Data storage
        self.gpu_utils = deque(maxlen=1000)
        self.gpu_memory = deque(maxlen=1000)
        self.cpu_utils = deque(maxlen=1000)
        self.ram_usage = deque(maxlen=1000)
        self.timestamps = deque(maxlen=1000)
    
    def get_gpu_stats(self):
        """Get GPU utilization and memory stats"""
        stats = []
        for i in range(self.device_count):
            handle = nvml.nvmlDeviceGetHandleByIndex(i)
            
            # Get utilization
            util = nvml.nvmlDeviceGetUtilizationRates(handle)
            
            # Get memory info
            mem_info = nvml.nvmlDeviceGetMemoryInfo(handle)
            
            stats.append({
                'gpu_util': util.gpu,
                'memory_used': mem_info.used / 1024**3,  # Convert to GB
                'memory_total': mem_info.total / 1024**3
            })
        
        return stats
    
    def log_stats(self):
        """Log system statistics"""
        while self.running:
            timestamp = time.time()
            
            # GPU stats
            gpu_stats = self.get_gpu_stats()
            if gpu_stats:
                self.gpu_utils.append(gpu_stats[0]['gpu_util'])
                self.gpu_memory.append(gpu_stats[0]['memory_used'])
            
            # CPU and RAM
            self.cpu_utils.append(psutil.cpu_percent())
            self.ram_usage.append(psutil.virtual_memory().used / 1024**3)
            self.timestamps.append(timestamp)
            
            time.sleep(self.log_interval)
    
    def start_monitoring(self):
        """Start monitoring in background thread"""
        self.running = True
        self.thread = threading.Thread(target=self.log_stats)
        self.thread.start()
        print("Training monitoring started...")
    
    def stop_monitoring(self):
        """Stop monitoring"""
        self.running = False
        if hasattr(self, 'thread'):
            self.thread.join()
        print("Training monitoring stopped.")
    
    def plot_stats(self, save_path=None):
        """Plot collected statistics"""
        if not self.timestamps:
            print("No data to plot")
            return
        
        fig, ((ax1, ax2), (ax3, ax4)) = plt.subplots(2, 2, figsize=(12, 8))
        
        times = [(t - self.timestamps[0]) / 60 for t in self.timestamps]  # Minutes from start
        
        # GPU Utilization
        ax1.plot(times, self.gpu_utils, 'b-')
        ax1.set_title('GPU Utilization (%)')
        ax1.set_ylabel('Utilization %')
        ax1.grid(True)
        
        # GPU Memory
        ax2.plot(times, self.gpu_memory, 'r-')
        ax2.set_title('GPU Memory Usage (GB)')
        ax2.set_ylabel('Memory (GB)')
        ax2.grid(True)
        
        # CPU Utilization
        ax3.plot(times, self.cpu_utils, 'g-')
        ax3.set_title('CPU Utilization (%)')
        ax3.set_xlabel('Time (minutes)')
        ax3.set_ylabel('Utilization %')
        ax3.grid(True)
        
        # RAM Usage
        ax4.plot(times, self.ram_usage, 'm-')
        ax4.set_title('RAM Usage (GB)')
        ax4.set_xlabel('Time (minutes)')
        ax4.set_ylabel('Memory (GB)')
        ax4.grid(True)
        
        plt.tight_layout()
        
        if save_path:
            plt.savefig(save_path, dpi=300, bbox_inches='tight')
            print(f"Statistics plot saved to {save_path}")
        
        plt.show()

# Usage
monitor = TrainingMonitor()
monitor.start_monitoring()

# ... run your training here ...

monitor.stop_monitoring()
monitor.plot_stats("training_stats.png")
```

### 2. Hyperparameter Optimization
```python
# optimize_hyperparams.py
import optuna
import json
import subprocess
import os
from quality_assessment import QualityAssessment

def objective(trial):
    """Optuna objective function for hyperparameter optimization"""
    
    # Suggest hyperparameters
    learning_rate = trial.suggest_float('learning_rate', 1e-6, 1e-3, log=True)
    network_dim = trial.suggest_categorical('network_dim', [16, 32, 64, 128])
    network_alpha = trial.suggest_categorical('network_alpha', [8, 16, 32, 64])
    noise_offset = trial.suggest_float('noise_offset', 0.0, 0.2)
    
    # Create configuration
    config = {
        "model_arguments": {
            "pretrained_model_name_or_path": "stabilityai/stable-diffusion-xl-base-1.0"
        },
        "training_arguments": {
            "learning_rate": learning_rate,
            "max_train_steps": 500,  # Shortened for optimization
            "mixed_precision": "fp16",
            "resolution": 1024
        },
        "lora_arguments": {
            "network_dim": network_dim,
            "network_alpha": network_alpha
        },
        "additional_arguments": {
            "noise_offset": noise_offset
        }
    }
    
    # Save temporary config
    config_path = f"temp_config_{trial.number}.json"
    with open(config_path, 'w') as f:
        json.dump(config, f)
    
    try:
        # Run training (simplified command)
        cmd = [
            "python", "train_network.py",
            "--config", config_path,
            "--output_dir", f"temp_output_{trial.number}"
        ]
        
        result = subprocess.run(cmd, capture_output=True)
        
        if result.returncode != 0:
            return 0.0  # Failed training
        
        # Assess quality
        lora_path = f"temp_output_{trial.number}/last.safetensors"
        if os.path.exists(lora_path):
            assessor = QualityAssessment(
                "stabilityai/stable-diffusion-xl-base-1.0",
                lora_path
            )
            
            test_prompts = ["test prompt for optimization"]
            assessment = assessor.assess_quality(test_prompts)
            
            # Return average CLIP score
            scores = [result['clip_score_mean'] for result in assessment.values()]
            return sum(scores) / len(scores)
        
        return 0.0
    
    finally:
        # Cleanup
        if os.path.exists(config_path):
            os.remove(config_path)

# Run optimization
study = optuna.create_study(direction='maximize')
study.optimize(objective, n_trials=20)

print("Best parameters:", study.best_params)
print("Best score:", study.best_value)
```

## Troubleshooting Common Issues

### Memory Issues
```bash
# Check VRAM usage during training
watch -n 1 nvidia-smi

# If running out of memory, try:
# 1. Reduce batch size
# 2. Enable gradient checkpointing
# 3. Use 8-bit optimizers
# 4. Lower resolution
# 5. Reduce network dimension
```

### Training Instability
```python
# Add to training config for stability
{
    "noise_offset": 0.1,
    "adaptive_noise_scale": 0.00357,
    "multires_noise_iterations": 10,
    "multires_noise_discount": 0.1,
    "lr_scheduler": "cosine_with_restarts",
    "lr_warmup_steps": 100
}
```

### Quality Issues
```python
# For overfitting:
# - Reduce learning rate
# - Add more diverse training data
# - Reduce training steps
# - Lower network rank

# For underfitting:
# - Increase learning rate
# - Increase training steps
# - Higher network rank
# - Better dataset quality
```

This implementation guide provides practical, ready-to-use code and configurations for setting up state-of-the-art LoRA training systems focused on generating highly realistic images.