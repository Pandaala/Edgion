---
name: cli-and-startup
description: 三个 bin 共同遵守的命令行约定、工作目录结构、配置文件路径规范。
---

# 统一命令行与配置约定

> **状态**: 框架已建立，待填充详细内容。

## 概要

三个 bin（edgion-controller、edgion-gateway、edgion-ctl）共享部分命令行设计哲学和配置约定。

## 待填充内容

### 命令行参数设计

<!-- TODO: 基于 clap 的 CLI 解析、通用参数（--config, --work-dir, --log-level 等） -->

### 工作目录结构

<!-- TODO: work_dir 规范、各 bin 在工作目录下创建的子目录 -->

### 配置文件路径

<!-- TODO: TOML 配置文件的加载顺序、默认路径、环境变量覆盖 -->

### 日志初始化

<!-- TODO: 三个 bin 共用的日志初始化流程 -->

### 信号处理

<!-- TODO: SIGTERM/SIGINT 的统一处理方式 -->
