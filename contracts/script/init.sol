import "forge-std/Script.sol";
import "forge-std/console.sol";
import "../src/Minting.sol";

contract Init is Script {
  function setUp() external {

  }

  function run() external {
    bytes memory deployCode = vm.getDeployedCode("Minting.sol");
    console.logBytes(deployCode);
  }
}

contract SimulateInteraction is Script {
  address public mintingContract;
  uint256 public testSigner;
  function setUp() external {
    mintingContract = vm.envAddress("MINTING_CONTRACT_ADDRESS");
    testSigner = vm.envUint("TEST_SIGNER_PRIV");
  }

  function run() external {
    // start broadcasting from test signer
    vm.startBroadcast(testSigner);


    // should try to burn 100 ethers (100 BTX)
    Minting(mintingContract).burn{value: 100 ether }(new bytes(0), new bytes(0));

    vm.stopBroadcast();
  }
}
