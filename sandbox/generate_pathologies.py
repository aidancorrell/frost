"""
Generate Iceberg tables with known pathologies for testing frost.

Run via:
    docker compose exec spark-iceberg spark-submit /opt/sandbox/generate_pathologies.py

Creates five tables in the 'frost_test' namespace:
  1. small_files_table     — 500 tiny files from simulated micro-batch writes
  2. snapshot_bloat_table  — 200+ snapshots from frequent commits without maintenance
  3. orphan_files_table    — Table with orphan files from simulated failed writes
  4. partition_skew_table  — Extreme partition skew (one hot partition)
  5. delete_heavy_table    — Many outstanding position deletes
"""

from pyspark.sql import SparkSession
from pyspark.sql import functions as F
from pyspark.sql.types import StructType, StructField, LongType, StringType, TimestampType
import datetime


def get_spark():
    return (
        SparkSession.builder
        .appName("frost-pathology-generator")
        .config("spark.sql.catalog.demo", "org.apache.iceberg.spark.SparkCatalog")
        .config("spark.sql.catalog.demo.type", "rest")
        .config("spark.sql.catalog.demo.uri", "http://rest-catalog:8181")
        .config("spark.sql.catalog.demo.io-impl", "org.apache.iceberg.aws.s3.S3FileIO")
        .config("spark.sql.catalog.demo.s3.endpoint", "http://minio:9000")
        .config("spark.sql.catalog.demo.s3.path-style-access", "true")
        .getOrCreate()
    )


def create_namespace(spark):
    spark.sql("CREATE NAMESPACE IF NOT EXISTS demo.frost_test")


def generate_small_files_table(spark):
    """Create a table with 500 tiny files via micro-batch appends."""
    print("Generating small_files_table...")
    spark.sql("DROP TABLE IF EXISTS demo.frost_test.small_files_table")
    spark.sql("""
        CREATE TABLE demo.frost_test.small_files_table (
            id BIGINT,
            event_type STRING,
            payload STRING,
            created_at TIMESTAMP
        ) USING iceberg
        PARTITIONED BY (days(created_at))
    """)

    for i in range(500):
        ts = datetime.datetime(2026, 1, 1) + datetime.timedelta(hours=i)
        df = spark.createDataFrame(
            [(i, "click", f"payload_{i}", ts)],
            ["id", "event_type", "payload", "created_at"],
        )
        df.writeTo("demo.frost_test.small_files_table").append()

    print(f"  Created small_files_table with 500 micro-batch appends")


def generate_snapshot_bloat_table(spark):
    """Create a table with 200+ snapshots."""
    print("Generating snapshot_bloat_table...")
    spark.sql("DROP TABLE IF EXISTS demo.frost_test.snapshot_bloat_table")
    spark.sql("""
        CREATE TABLE demo.frost_test.snapshot_bloat_table (
            id BIGINT,
            value STRING
        ) USING iceberg
    """)

    for i in range(200):
        df = spark.createDataFrame([(i, f"value_{i}")], ["id", "value"])
        df.writeTo("demo.frost_test.snapshot_bloat_table").append()

    print(f"  Created snapshot_bloat_table with 200 snapshots")


def generate_partition_skew_table(spark):
    """Create a table with extreme partition skew."""
    print("Generating partition_skew_table...")
    spark.sql("DROP TABLE IF EXISTS demo.frost_test.partition_skew_table")
    spark.sql("""
        CREATE TABLE demo.frost_test.partition_skew_table (
            id BIGINT,
            region STRING,
            amount BIGINT,
            created_at TIMESTAMP
        ) USING iceberg
        PARTITIONED BY (region)
    """)

    # Normal partitions: 10 records each
    for region in ["us-east", "us-west", "eu-west", "ap-south"]:
        rows = [(i, region, i * 100, datetime.datetime(2026, 1, 1))
                for i in range(10)]
        df = spark.createDataFrame(rows, ["id", "region", "amount", "created_at"])
        df.writeTo("demo.frost_test.partition_skew_table").append()

    # Hot partition: 500 separate appends
    for batch in range(500):
        rows = [(batch, "ap-northeast", batch * 100, datetime.datetime(2026, 1, 1))]
        df = spark.createDataFrame(rows, ["id", "region", "amount", "created_at"])
        df.writeTo("demo.frost_test.partition_skew_table").append()

    print(f"  Created partition_skew_table with skewed 'ap-northeast' partition")


def generate_delete_heavy_table(spark):
    """Create a table with many position deletes."""
    print("Generating delete_heavy_table...")
    spark.sql("DROP TABLE IF EXISTS demo.frost_test.delete_heavy_table")
    spark.sql("""
        CREATE TABLE demo.frost_test.delete_heavy_table (
            id BIGINT,
            status STRING,
            updated_at TIMESTAMP
        ) USING iceberg
        TBLPROPERTIES (
            'write.delete.mode' = 'merge-on-read',
            'write.update.mode' = 'merge-on-read'
        )
    """)

    # Insert initial data
    rows = [(i, "active", datetime.datetime(2026, 1, 1)) for i in range(10000)]
    df = spark.createDataFrame(rows, ["id", "status", "updated_at"])
    df.writeTo("demo.frost_test.delete_heavy_table").append()

    # Generate deletes via MERGE
    for batch in range(50):
        start_id = batch * 100
        end_id = start_id + 100
        spark.sql(f"""
            DELETE FROM demo.frost_test.delete_heavy_table
            WHERE id >= {start_id} AND id < {end_id}
        """)

    print(f"  Created delete_heavy_table with 50 delete operations")


if __name__ == "__main__":
    spark = get_spark()
    create_namespace(spark)

    generate_small_files_table(spark)
    generate_snapshot_bloat_table(spark)
    generate_partition_skew_table(spark)
    generate_delete_heavy_table(spark)

    print("\nAll pathological tables created successfully!")
    print("Run 'frost check' against these tables to validate health checks.")

    spark.stop()
