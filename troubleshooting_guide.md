# LoRA Local Setup Troubleshooting Guide

## Common Installation Issues

### Python Environment Problems

#### Issue: "Python not found" or "python: command not found"
**Solutions:**
1. **Windows**: Add Python to PATH during installation or manually:
   - Control Panel → System → Advanced → Environment Variables
   - Add Python installation directory to PATH
2. **Linux/Mac**: Install Python via package manager:
   ```bash
   # Ubuntu/Debian
   sudo apt install python3 python3-pip python3-venv
   
   # macOS
   brew install python@3.11
   ```

#### Issue: Virtual environment activation fails
**Solutions:**
- **Windows PowerShell execution policy**:
  ```powershell
  Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
  ```
- **Linux/Mac permissions**:
  ```bash
  chmod +x lora_env/bin/activate
  source lora_env/bin/activate
  ```

#### Issue: pip install failures
**Solutions:**
1. Upgrade pip: `pip install --upgrade pip`
2. Clear cache: `pip cache purge`
3. Use alternative index: `pip install --index-url https://pypi.org/simple/ package_name`
4. Install with no dependencies first: `pip install --no-deps package_name`

---

## GPU and CUDA Issues

### NVIDIA GPU Problems

#### Issue: "CUDA out of memory" during training/inference
**Immediate Solutions:**
```bash
# Training
--train_batch_size=1
--gradient_accumulation_steps=4
--mixed_precision="fp16"

# Inference
--medvram  # or --lowvram for severe cases
--xformers
--opt-split-attention
```

**Advanced Solutions:**
1. **Reduce model precision**: Use fp16 instead of fp32
2. **Enable gradient checkpointing**: Trade compute for memory
3. **Use CPU offloading**: For models with `--cpu` flag
4. **Reduce resolution**: Train/generate at 512x512 instead of 1024x1024

#### Issue: "RuntimeError: No CUDA GPUs are available"
**Solutions:**
1. **Check NVIDIA drivers**:
   ```bash
   nvidia-smi  # Should show GPU info
   ```
2. **Install/update CUDA toolkit**:
   - Download from https://developer.nvidia.com/cuda-downloads
   - Match CUDA version with PyTorch version
3. **Reinstall PyTorch with CUDA**:
   ```bash
   pip uninstall torch torchvision torchaudio
   pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu124
   ```

#### Issue: "cuDNN error" or "CUDA initialization failed"
**Solutions:**
1. **Update GPU drivers**: Use NVIDIA GeForce Experience or manual download
2. **Check GPU compatibility**: Ensure GPU supports required CUDA compute capability
3. **Restart system**: After driver updates
4. **Check for conflicts**: Multiple CUDA versions or mining software

### AMD GPU Issues

#### Issue: ROCm installation problems (Linux)
**Solutions:**
1. **Install ROCm properly**:
   ```bash
   # Ubuntu 22.04
   wget https://repo.radeon.com/amdgpu-install/latest/ubuntu/jammy/amdgpu-install_latest_all.deb
   sudo dpkg -i amdgpu-install_latest_all.deb
   sudo amdgpu-install --usecase=rocm
   ```
2. **Add user to render group**:
   ```bash
   sudo usermod -a -G render,video $LOGNAME
   ```
3. **Set environment variables**:
   ```bash
   export HSA_OVERRIDE_GFX_VERSION=10.3.0  # For RX 6000 series
   export HSA_OVERRIDE_GFX_VERSION=11.0.0  # For RX 7000 series
   ```

#### Issue: AMD GPU not detected on Windows
**Solutions:**
1. AMD ROCm for Windows is experimental - consider using CPU mode
2. Try DirectML backend: `pip install torch-directml`
3. Use OpenVINO for Intel integrated graphics

---

## Training-Specific Issues

### LoRA Training Problems

#### Issue: Training loss not decreasing
**Solutions:**
1. **Check dataset quality**:
   - Ensure images are high resolution and consistent
   - Verify captions are accurate and detailed
   - Remove blurry or low-quality images

2. **Adjust learning parameters**:
   ```bash
   # Increase learning rate
   --learning_rate=2e-4  # from 1e-4
   
   # Increase LoRA rank
   --network_dim=256  # from 128
   
   # Adjust alpha
   --network_alpha=256  # usually same as dim
   ```

3. **Extend training duration**:
   ```bash
   --max_train_steps=3000  # from 1500
   ```

#### Issue: Overfitting (loss drops too fast, then plateaus)
**Solutions:**
1. **Add regularization**:
   ```bash
   --prior_loss_weight=1.0
   --reg_data_dir=./regularization_images
   ```

2. **Reduce learning rate**:
   ```bash
   --learning_rate=5e-5  # from 1e-4
   ```

3. **Use learning rate scheduling**:
   ```bash
   --lr_scheduler="cosine_with_restarts"
   --lr_warmup_steps=100
   ```

#### Issue: "nan loss" or training crashes
**Solutions:**
1. **Use mixed precision carefully**:
   ```bash
   --mixed_precision="fp16"
   --save_precision="fp16"
   ```

2. **Lower learning rate significantly**:
   ```bash
   --learning_rate=1e-5
   ```

3. **Check for corrupted data**:
   - Verify all images can be opened
   - Ensure consistent image formats
   - Check caption files exist and are readable

### Dataset Issues

#### Issue: "No training data found"
**Solutions:**
1. **Check directory structure**:
   ```
   training_data/
   ├── images/
   │   ├── 001_image.jpg
   │   └── 002_image.png
   └── captions/  # or .txt files with same names
       ├── 001_image.txt
       └── 002_image.txt
   ```

2. **Verify file permissions**: Ensure read access to all files

3. **Check supported formats**: Use JPG, PNG, WebP formats

#### Issue: Poor training results despite good data
**Solutions:**
1. **Improve caption quality**:
   - Be specific and detailed
   - Include relevant style descriptors
   - Use consistent terminology

2. **Balance dataset**:
   - Include diverse poses/angles
   - Maintain consistent subject/style
   - Aim for 50-200 high-quality images

3. **Preprocessing optimization**:
   ```bash
   --resolution=512  # or 768 for SDXL
   --bucket_resolution_steps=64
   --cache_latents  # Speed up training
   ```

---

## Inference and Generation Issues

### Image Quality Problems

#### Issue: Blurry or low-quality outputs
**Solutions:**
1. **Improve prompts**:
   - Add quality tags: "masterpiece, best quality, highly detailed"
   - Be more specific about desired style
   - Use negative prompts to exclude unwanted elements

2. **Adjust generation parameters**:
   ```bash
   --cfg_scale=7.5  # Balance between creativity and adherence
   --steps=20-30    # More steps = higher quality (diminishing returns)
   --sampler="DPM++ 2M Karras"  # Often produces good results
   ```

3. **Check model quality**: Ensure using a good base model (SD 1.5, SDXL, etc.)

#### Issue: LoRA effects not visible
**Solutions:**
1. **Increase LoRA strength**:
   ```python
   pipeline.load_lora_weights("path/to/lora", weight_name="file.safetensors")
   # In ComfyUI: increase LoRA strength from 1.0 to 1.2-1.5
   ```

2. **Use trigger words**: Include specific terms the LoRA was trained on

3. **Check LoRA compatibility**: Ensure LoRA matches base model (SD 1.5 LoRA with SD 1.5 base)

### Performance Issues

#### Issue: Slow generation times
**Solutions:**
1. **Enable optimizations**:
   ```bash
   --xformers  # Memory efficient attention
   --opt-sdp-attention  # PyTorch 2.0+ optimization
   --opt-channelslast  # Memory layout optimization
   ```

2. **Use appropriate precision**:
   ```bash
   --precision=autocast  # Automatic mixed precision
   --no-half-vae  # If getting NaN values
   ```

3. **Optimize batch sizes**: Generate multiple images at once for efficiency

#### Issue: Running out of VRAM during inference
**Solutions (in order of preference)**:
1. **Enable memory optimizations**:
   ```bash
   --medvram     # Split model into parts
   --lowvram     # Most aggressive memory saving
   --opt-sub-quad-attention  # Alternative attention mechanism
   ```

2. **Reduce generation parameters**:
   - Lower resolution (512x512 instead of 1024x1024)
   - Fewer inference steps
   - Smaller batch size

3. **Use CPU fallback**:
   ```bash
   --use-cpu=all  # Use CPU for all operations (slow but works)
   ```

---

## Software-Specific Issues

### ComfyUI Problems

#### Issue: Nodes not loading or missing
**Solutions:**
1. **Update ComfyUI**:
   ```bash
   git pull origin master
   ```

2. **Install missing custom nodes**:
   - Use ComfyUI Manager extension
   - Manually install from GitHub repositories

3. **Clear node cache**: Delete `web/extensions` folder and restart

#### Issue: Workflow errors
**Solutions:**
1. **Check node connections**: Ensure correct input/output types
2. **Verify model paths**: Models must be in correct folders
3. **Update workflows**: Older workflows may not work with newer ComfyUI versions

### AUTOMATIC1111 WebUI Problems

#### Issue: Extensions not working
**Solutions:**
1. **Update WebUI**:
   ```bash
   git pull
   ```

2. **Reinstall extensions**: Go to Extensions tab → Installed → Check for updates

3. **Check compatibility**: Some extensions may not work with recent WebUI versions

#### Issue: "RuntimeError: Couldn't install torch" 
**Solutions:**
1. **Manual PyTorch installation**:
   ```bash
   pip install torch==2.6.0 torchvision torchaudio --index-url https://download.pytorch.org/whl/cu124
   ```

2. **Clear pip cache**: `pip cache purge`

3. **Use different PyTorch version**: Try CUDA 11.8 build if 12.4 fails

---

## Network and Download Issues

### Model Download Problems

#### Issue: "Connection timeout" or "Download failed"
**Solutions:**
1. **Use mirrors**:
   - HuggingFace Hub mirror sites
   - Direct download links instead of git-lfs

2. **Download manually**:
   ```bash
   wget -O model.safetensors "https://huggingface.co/repo/model/resolve/main/model.safetensors"
   ```

3. **Use resume capability**:
   ```bash
   wget -c -O model.safetensors "url"  # Continue interrupted download
   ```

#### Issue: Git LFS bandwidth limits
**Solutions:**
1. **Use direct links**: Download .safetensors files directly instead of git clone
2. **Use alternative hosting**: Some models available on Civitai or other sites
3. **Wait and retry**: LFS quotas reset monthly

---

## Performance Optimization Checklist

### Before Training
- [ ] GPU drivers updated
- [ ] CUDA/ROCm properly installed
- [ ] Virtual environment activated
- [ ] Dataset validated and preprocessed
- [ ] Adequate storage space available
- [ ] System monitoring tools ready (htop, nvidia-smi)

### During Training
- [ ] Monitor GPU utilization and temperature
- [ ] Watch for memory leaks
- [ ] Validate loss progression
- [ ] Save checkpoints regularly
- [ ] Monitor training samples

### After Training
- [ ] Test LoRA with different prompts
- [ ] Validate LoRA strength settings
- [ ] Check for overfitting
- [ ] Archive training data and logs
- [ ] Document successful parameters

---

## Emergency Fixes

### Complete Reset Procedure
If everything breaks and you need to start fresh:

1. **Backup important files**:
   ```bash
   # Save any trained LoRAs, custom models, or datasets
   cp -r ./output/trained_loras ./backup/
   ```

2. **Remove Python environment**:
   ```bash
   rm -rf lora_env/  # Linux/Mac
   rmdir /s lora_env  # Windows
   ```

3. **Clean installations**:
   ```bash
   rm -rf ComfyUI/ stable-diffusion-webui/ kohya-scripts/
   ```

4. **Reinstall from scratch**:
   - Run setup script again
   - Restore backed up files

### Quick Diagnostic Commands

```bash
# Check GPU status
nvidia-smi

# Check Python environment
python --version
pip list | grep torch

# Check disk space
df -h  # Linux/Mac
dir   # Windows

# Check memory usage
free -h  # Linux
top      # Linux/Mac
tasklist # Windows

# Test basic functionality
python -c "import torch; print(torch.cuda.is_available())"
```

---

## Getting Help

### Log Files to Check
- **Training logs**: `./logs/` directory
- **ComfyUI logs**: Console output and `comfyui.log`
- **WebUI logs**: Console output during startup
- **System logs**: Check for GPU driver issues

### Information to Gather Before Asking for Help
1. **System specifications**: GPU model, VRAM, OS version
2. **Software versions**: Python, PyTorch, CUDA versions
3. **Exact error messages**: Copy complete error text
4. **Steps to reproduce**: What you were doing when error occurred
5. **Recent changes**: What was installed or changed recently

### Community Resources
- **GitHub Issues**: Check repository issue trackers
- **Discord/Reddit**: Active communities for each tool
- **Documentation**: Official docs and wikis
- **Forums**: Civitai community, HuggingFace forums

Remember: Most issues are environment-related and can be resolved by carefully following installation instructions and ensuring all dependencies are properly installed.