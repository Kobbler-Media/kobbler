# kobbler-tools: pre-built base image with heavy runtime dependencies.
# Avoids re-downloading ~400MB calibre on every --no-cache build.
#
# Rebuild with: make tools
#
# Base: debian:sid + deb-multimedia for ffmpeg with libfdk_aac.
# We want ffmpeg ≥7.0 (kob-ffmpeg-sys uses AVBufferSrcParameters.side_data)
# and libfdk_aac (--enable-nonfree, materially better AAC than native at
# kobbler's 256k HLS default). deb-multimedia ships exactly that on sid as
# `ffmpeg 8.1` — no from-source build needed. Was previously a multi-stage
# from-source compile because BtbN's GPL build excludes libfdk_aac.

FROM debian:sid-slim

ENV DEBIAN_FRONTEND=noninteractive

# Add deb-multimedia repo (non-free ffmpeg with libfdk_aac).
# Their keyring package signs the rest; install the keyring first via the
# minimal stage that has curl, then bring up the full toolchain.
RUN apt-get update && apt-get install -y \
        ca-certificates curl gnupg \
    && curl -fsSL https://www.deb-multimedia.org/pool/main/d/deb-multimedia-keyring/deb-multimedia-keyring_2024.9.1_all.deb -o /tmp/dmo.deb \
    && apt-get install -y /tmp/dmo.deb \
    && echo "deb https://www.deb-multimedia.org sid main non-free" > /etc/apt/sources.list.d/deb-multimedia.list \
    && rm /tmp/dmo.deb \
    && rm -rf /var/lib/apt/lists/*

# Heavy runtime + build deps:
# - ca-certificates, curl: HTTPS
# - calibre: ebook-convert + ebook-meta — fallback for PDF/FB2/RTF/TXT/HTML/DOCX/LIT/PDB
# - mesa-va-drivers, libva*: VAAPI hardware encoding support (Intel/AMD GPUs)
# - cmake + libfftw3-dev: chromaprint built from source below
# - pkg-config + libclang-dev: kob-ffmpeg-sys bindgen at compile time
# - ffmpeg + libav*-dev: from deb-multimedia, includes libfdk_aac
# - python3 + venv: faster-whisper transcription (audiobook STT)
RUN apt-get update && apt-get install -y \
        ca-certificates calibre curl xz-utils \
        cmake libfftw3-dev libclang-dev pkg-config \
        ffmpeg libavcodec-dev libavformat-dev libavfilter-dev libavutil-dev \
        libavdevice-dev libswresample-dev libswscale-dev \
        libva2 libva-drm2 mesa-libgallium \
        python3 python3-pip python3-full python3-venv \
        build-essential \
    && rm -rf /var/lib/apt/lists/*

# Install Rust toolchain via rustup (sid's apt rust is older than our 1.95
# rust-version requirement). Pin a known version so rebuilds are reproducible.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    RUST_VERSION=1.96.0
RUN curl -fsSL https://sh.rustup.rs | sh -s -- \
        --default-toolchain $RUST_VERSION --profile minimal -y \
    && rustc --version \
    && cargo --version

# Build chromaprint 1.6.1 from source (latest official release).
# Uses FFTW3 for better accuracy. Installed to /usr/local alongside FFmpeg.
RUN curl -fsSL --retry 5 --retry-all-errors https://github.com/acoustid/chromaprint/releases/download/v1.6.1/chromaprint-1.6.1.tar.gz \
    | tar -xz \
    && cd chromaprint-1.6.1 \
    && cmake -B build -DCMAKE_INSTALL_PREFIX=/usr/local -DCMAKE_BUILD_TYPE=Release -DBUILD_TOOLS=OFF -DBUILD_SHARED_LIBS=ON \
    && cmake --build build --parallel \
    && cmake --install build \
    && cd / && rm -rf chromaprint-1.6.1 \
    && ldconfig

# Chromaprint's cmake doesn't install a pkg-config file — create one manually.
RUN test -f /usr/local/lib/pkgconfig/libchromaprint.pc || \
    printf 'prefix=/usr/local\nexec_prefix=${prefix}\nlibdir=${prefix}/lib\nincludedir=${prefix}/include\n\nName: chromaprint\nDescription: Audio fingerprint library\nVersion: 1.6.1\nLibs: -L${libdir} -lchromaprint\nCflags: -I${includedir}\n' \
    > /usr/local/lib/pkgconfig/libchromaprint.pc

# Make sure pkg-config sees both /usr/local and the system paths.
ENV PKG_CONFIG_PATH=/usr/local/lib/pkgconfig:/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig

# faster-whisper venv — used for audiobook transcription (parakeet-stt is the
# main path; this is a CPU fallback / shorter-clip helper).
ENV VIRTUAL_ENV=/opt/venv
RUN python3 -m venv $VIRTUAL_ENV
ENV PATH=$VIRTUAL_ENV/bin:$PATH
RUN pip install --no-cache-dir faster-whisper \
    && python3 -c "from faster_whisper import WhisperModel; WhisperModel('base.en', device='cpu', compute_type='int8')"

# cargo-watch for dev hot-reload
RUN cargo install cargo-watch
