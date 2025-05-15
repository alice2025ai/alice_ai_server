-- 添加metadata列到sync_status表
ALTER TABLE sync_status ADD COLUMN IF NOT EXISTS metadata TEXT;

-- 创建索引以提高查询性能
CREATE INDEX IF NOT EXISTS idx_sync_status_metadata ON sync_status(metadata);

-- 更新注释
COMMENT ON COLUMN sync_status.metadata IS '存储同步状态的额外元数据，如Sui区块链的完整cursor信息'; 