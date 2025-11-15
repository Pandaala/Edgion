# Edgion


todolist


plugins
- 内部增加一个配置，is_record_time_cost()函数，如果有的话，就在插件前后增加耗时计算




# 使用配置文件
./edgion-op --config edgion/config/edgion-op.toml

# 使用默认配置
./edgion-op

# 命令行参数覆盖配置文件
./edgion-op -c edgion/config/edgion-op.toml --log-level debug --grpc-listen "0.0.0.0:50052"

# 查看帮助
./edgion-op --help